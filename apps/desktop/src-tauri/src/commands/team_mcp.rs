//! Session-local HTTP access to the existing team dispatcher.
use super::CommandResult;
use crate::state::AppState;
use serde_json::Value;
use std::sync::Arc;
use thingmaker_supervisor::{DesktopError, agents::Provider, supervisor::SessionActorConfig};

/// Copilot receives HTTP MCP servers through ACP, for leaders and workers alike.
pub async fn prepare(state: &AppState, config: &mut SessionActorConfig) -> CommandResult<()> {
    if config.launch.provider() != Provider::Copilot { return Ok(()) }
    if let Some(entry) = config.mcp_servers.iter_mut().find(|entry| entry["name"] == thingmaker_supervisor::delegation::mcp::SERVER_NAME && entry["command"].is_string()) {
        let delegation = state.delegation.get().cloned().ok_or_else(|| DesktopError::not_ready("Team service unavailable"))?;
        config.local_team_lifetime = Some(endpoint(delegation, entry).await?);
    }
    Ok(())
}

/// The built-in team server already lives inside the desktop. Expose its
/// existing dispatcher over loopback HTTP without starting a relay process.
pub(super) async fn endpoint(delegation: thingmaker_supervisor::delegation::Delegation, entry: &mut Value) -> CommandResult<Arc<tokio::sync::oneshot::Sender<()>>> {
    use axum::{extract::DefaultBodyLimit, http::{HeaderMap, StatusCode}, response::IntoResponse, routing::post, Json, Router};
    use thingmaker_supervisor::delegation::{mcp, socket::TOKEN_ENV};
    let token = entry["env"].as_array().into_iter().flatten().find(|pair| pair["name"] == TOKEN_ENV)
        .and_then(|pair| pair["value"].as_str()).ok_or_else(|| DesktopError::not_ready("Missing team credential"))?.to_owned();
    let auth = format!("Bearer {token}");
    let expected = auth.clone();
    let server_token = token.clone();
    let app = Router::new().route("/mcp", post(move |headers: HeaderMap, Json(message): Json<Value>| {
        let caller = if !headers.contains_key("origin") && headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&expected) {
            delegation.caller_for_token(&token)
        } else { None };
        let delegation = delegation.clone();
        async move {
            let Some(caller) = caller else { return StatusCode::FORBIDDEN.into_response() };
            match mcp::handle(&delegation, &caller, &message).await {
                Some(reply) => Json(reply).into_response(),
                None => StatusCode::ACCEPTED.into_response(),
            }
        }
    })).layer(DefaultBodyLimit::max(1024 * 1024));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.map_err(|e| DesktopError::io(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| DesktopError::io(e.to_string()))?.port();
    let (owner, closed) = tokio::sync::oneshot::channel();
    tokio::spawn(async move { let _ = axum::serve(listener, app).with_graceful_shutdown(async { let _ = closed.await; }).await; });
    *entry = thingmaker_supervisor::delegation::socket::http_server_entry(&format!("http://127.0.0.1:{port}/mcp"), &server_token);
    Ok(Arc::new(owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{sync::Mutex, time::Duration};
    use thingmaker_supervisor::{
        acp::LaunchTarget,
        agents::{AgentLaunch, copilot::CopilotLaunchOptions},
        delegation::{Combo, Delegation, WorkerLauncher, WorkerSlot, WorkerSpec, jobs::{BoxFuture, DelegateArgs}, socket::{self, DelegationSocket}},
        supervisor::SessionActor,
    };

    struct WorkerToken {
        sent: Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
        finish: Arc<tokio::sync::Notify>,
    }

    impl WorkerLauncher for WorkerToken {
        fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
            assert_eq!(spec.slot.provider, Provider::Copilot);
            if let Some(sent) = self.sent.lock().unwrap().take() { let _ = sent.send(spec.mcp_token.expect("a reduced team token")); }
            let finish = self.finish.clone();
            Box::pin(async move { finish.notified().await; Err(DesktopError::not_ready("test worker released")) })
        }
        fn released(&self, _: &str) {}
    }

    fn config(root: &std::path::Path) -> SessionActorConfig {
        SessionActorConfig::new(LaunchTarget::executable("copilot"), AgentLaunch::Copilot(CopilotLaunchOptions::new(root)))
    }

    async fn request(entry: &Value, auth: &str, origin: bool, method: &str) -> (u16, Value) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let address = entry["url"].as_str().unwrap().strip_prefix("http://").unwrap().strip_suffix("/mcp").unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let body = json!({"jsonrpc":"2.0","id":1,"method":method}).to_string();
            let origin = if origin { "Origin: http://localhost\r\n" } else { "" };
            let headers = format!("POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAuthorization: {auth}\r\n{origin}Connection: close\r\nContent-Length: {}\r\n\r\n", body.len());
            stream.write_all(format!("{headers}{body}").as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, serde_json::from_str(body).unwrap_or(Value::Null))
        }).await.unwrap()
    }

    async fn check_endpoint(entry: &Value, worker: bool) {
        assert_eq!(entry["name"], "team");
        assert_eq!(entry["type"], "http");
        assert!(entry.get("command").is_none() && entry.get("env").is_none());
        assert_eq!(entry["headers"][0]["name"], "Authorization");
        let auth = entry["headers"][0]["value"].as_str().unwrap();
        assert!(auth.starts_with("Bearer "));
        assert!(entry["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
        assert_eq!(request(entry, "Bearer wrong", false, "tools/list").await.0, 403);
        assert_eq!(request(entry, auth, true, "tools/list").await.0, 403);
        let (status, tools) = request(entry, auth, false, "tools/list").await;
        assert_eq!(status, 200);
        let names: Vec<_> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
        assert_eq!(names.contains(&"delegate"), !worker, "{names:?}");
        assert_eq!(names.contains(&"list_workers"), !worker, "{names:?}");
        let (status, initialized) = request(entry, auth, false, "initialize").await;
        assert_eq!(status, 200);
        let instructions = initialized["result"]["instructions"].as_str().unwrap();
        assert_eq!(instructions.contains("this worker"), worker, "{instructions}");
    }

    #[tokio::test]
    async fn copilot_leader_registration_reuses_the_authenticated_team_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let delegation = Delegation::new(Arc::new(WorkerToken { sent: Mutex::new(None), finish: Default::default() }), |_| {});
        let socket = DelegationSocket::bind(delegation.clone(), socket::socket_path(dir.path())).unwrap();
        assert!(state.delegation.set(delegation.clone()).is_ok());
        assert!(state.delegation_socket.set(socket).is_ok());
        let mut config = config(dir.path());
        let mut launch = config.launch.clone();
        assert!(super::super::delegation::prepare_orchestrator(&state, "lead", dir.path().into(), &Combo::default(), &mut launch, &mut config, false));
        config.launch = launch;
        assert!(config.leads_team);
        let AgentLaunch::Copilot(options) = &config.launch else { unreachable!() };
        let guidance = options.initial_prompt_prefix.as_deref().unwrap();
        assert!(guidance.contains("list_workers") && !guidance.contains("tool is turned off"));
        prepare(&state, &mut config).await.unwrap();
        assert!(config.local_team_lifetime.is_some());
        check_endpoint(&config.mcp_servers[0], false).await;
        // Preparing twice must not replace an attachment's endpoint or token.
        let before = config.mcp_servers.clone();
        prepare(&state, &mut config).await.unwrap();
        assert_eq!(config.mcp_servers, before);
        delegation.release("lead").await;
        let entry = &config.mcp_servers[0];
        assert_eq!(request(entry, entry["headers"][0]["value"].as_str().unwrap(), false, "tools/list").await.0, 403, "released tokens no longer authorize requests");
    }

    #[tokio::test]
    async fn a_copilot_worker_token_exposes_only_the_reduced_team_tools() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let finish = Arc::new(tokio::sync::Notify::new());
        let delegation = Delegation::new(Arc::new(WorkerToken { sent: Mutex::new(Some(sent)), finish: finish.clone() }), |_| {});
        let combo = Combo { workers: vec![WorkerSlot { name: "copilot-worker".into(), provider: Provider::Copilot, model: None, effort: None, capabilities: Vec::new(), note: None, prompt: None, resources: None }], ..Default::default() };
        let lead_token = delegation.register("lead", Provider::Claude, dir.path().into(), combo);
        assert!(state.delegation.set(delegation.clone()).is_ok());
        let job = delegation.delegate("lead", DelegateArgs { task: "Test the worker token".into(), worker: Some("copilot-worker".into()), ..Default::default() }).unwrap();
        let token = tokio::time::timeout(Duration::from_secs(10), received).await.unwrap().unwrap();
        assert_ne!(token, lead_token);
        let mut config = config(dir.path());
        config.mcp_servers = vec![socket::server_entry(dir.path(), &[], dir.path(), &token)];
        prepare(&state, &mut config).await.unwrap();
        assert!(!config.leads_team);
        check_endpoint(&config.mcp_servers[0], true).await;
        finish.notify_one();
        delegation.await_jobs("lead", &[job.id], Duration::from_secs(10), false).await.unwrap();
        delegation.release("lead").await;
    }
}

