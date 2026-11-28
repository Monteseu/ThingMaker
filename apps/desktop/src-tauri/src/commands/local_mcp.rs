//! Make selected stdio servers reachable by a Windows AppContainer harness.
//! Each child still runs inside the same AppContainer. Credentials and config
//! travel through inherited pipes, and the actor owns the bridge lifetime.
use super::CommandResult;
use crate::state::AppState;
use serde_json::{Value, json};
use std::{process::Stdio, sync::Arc, time::Duration};
use thingmaker_supervisor::{DesktopError, agents::{AgentLaunch, Provider}, supervisor::SessionActorConfig};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, process::Command};


pub async fn prepare(state: &AppState, config: &mut SessionActorConfig) -> CommandResult<()> {
    if config.launch.provider() != Provider::Local || matches!(&config.launch, AgentLaunch::Claude(o) if o.local.as_ref().is_some_and(|t| t.kind == thingmaker_supervisor::agents::local::LocalKind::Chat)) {
        return Ok(());
    }
    if let Some(team) = config.mcp_servers.iter_mut().find(|entry| entry["name"] == thingmaker_supervisor::delegation::mcp::SERVER_NAME && entry["command"].is_string()) {
        config.local_team_lifetime = Some(super::team_mcp::endpoint(state.delegation.get().cloned().ok_or_else(|| DesktopError::not_ready("Team service unavailable"))?, team).await?);
    }
    let workspace = config.launch.root().to_owned();
    let mut environment = config.environment.resolve(std::env::vars());
    // A sandboxed server needs its writable temp folder too.
    for (key, value) in config.launch.environment().into_iter().filter(|(key, _)| key == "TEMP" || key == "TMP") {
        environment.insert(key, value);
    }
    let optional = match &mut config.launch {
        AgentLaunch::Claude(o) => o.resources.as_mut().map(|(_, servers)| servers),
        AgentLaunch::Codex(o) => o.resources.as_mut().map(|resources| &mut resources.mcps),
        _ => None,
    };
    let mut servers: Vec<_> = optional.into_iter().flat_map(|servers| servers.values_mut())
        .map(|value| (value, false)).chain(config.mcp_servers.iter_mut().map(|value| (value, true)))
        .filter(|(value, _)| value["command"].is_string()).collect();
    if servers.is_empty() { return Ok(()) }
    let specs: Vec<_> = servers.iter().map(|(value, acp)| {
        let mut spec = (**value).clone();
        if *acp {
            let env: serde_json::Map<_, _> = spec["env"].as_array().into_iter().flatten()
                .filter_map(|pair| Some((pair["name"].as_str()?.to_owned(), pair["value"].clone()))).collect();
            spec["env"] = Value::Object(env);
        }
        spec
    }).collect();
    let runtime = state.resource_dir.as_ref().map(|dir| dir.join("local-mcp-runtime"));
    #[cfg(debug_assertions)]
    let runtime = runtime.filter(|dir| dir.join("node.exe").is_file())
        .or_else(|| Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist-local-mcp")));
    let runtime = runtime.and_then(|dir| dunce::canonicalize(dir).ok()).filter(|dir| dir.join("node.exe").is_file())
        .ok_or_else(|| DesktopError::not_ready("The local MCP runtime is missing. Reinstall ThingMaker."))?;
    let launcher = std::env::current_exe().map_err(|e| DesktopError::io(e.to_string()))?;
    let launch = json!({ "launcher": launcher, "workspace": workspace, "environment": environment, "servers": specs });
    let mut child = Command::new(runtime.join("node.exe"))
        .args(["-e", include_str!("../../../../../runtime/agents/local-mcp-bridge.cjs")])
        .current_dir(runtime).env_clear().envs(thingmaker_supervisor::security::EnvironmentProfile::trusted_local().resolve(std::env::vars()))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
        .creation_flags(0x08000000).kill_on_drop(true).spawn().map_err(|e| DesktopError::io(e.to_string()))?;
    let mut input = child.stdin.take().ok_or_else(|| DesktopError::io("Missing MCP bridge control pipe"))?;
    let output = child.stdout.take().ok_or_else(|| DesktopError::io("Missing MCP bridge reply pipe"))?;
    input.write_all(format!("{launch}\n").as_bytes()).await.map_err(|e| DesktopError::io(e.to_string()))?;
    let line = tokio::time::timeout(Duration::from_secs(10), BufReader::new(output).lines().next_line()).await
        .map_err(|_| DesktopError::not_ready("Local MCP bridge startup timed out"))?.map_err(|e| DesktopError::io(e.to_string()))?
        .ok_or_else(|| DesktopError::not_ready("Local MCP bridge exited during startup"))?;
    let endpoints: Vec<Value> = serde_json::from_str(&line).map_err(|_| DesktopError::io("Invalid local MCP bridge reply"))?;
    if servers.len() != endpoints.len() { return Err(DesktopError::io("Missing local MCP endpoints")) }
    for ((server, acp), mut endpoint) in servers.drain(..).zip(endpoints) {
        if acp {
            endpoint["name"] = server["name"].clone();
            endpoint["headers"] = json!([{"name":"Authorization","value":endpoint["headers"]["Authorization"]}]);
        }
        *server = endpoint;
    }
    config.local_mcp_bridge = Some(Arc::new(input));
    tokio::spawn(async move { let _ = child.wait().await; });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use thingmaker_supervisor::{acp::LaunchTarget, agents::codex::CodexLaunchOptions,
        delegation::{Delegation, WorkerLauncher, WorkerSpec, jobs::BoxFuture, socket}, supervisor::SessionActor};
    use tokio::io::AsyncReadExt;

    struct UnusedLauncher;
    impl WorkerLauncher for UnusedLauncher {
        fn launch(&self, _: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> { unreachable!() }
        fn released(&self, _: &str) {}
    }

    #[tokio::test]
    async fn the_local_team_answers_authenticated_discovery_without_a_stdio_bridge() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::initialize(temp.path(), None).unwrap();
        let delegation = Delegation::new(Arc::new(UnusedLauncher), |_| {});
        let token = delegation.register("test", Provider::Local, temp.path().into(), Default::default());
        state.delegation.set(delegation).unwrap();
        let mut options = CodexLaunchOptions::new(temp.path(), "codex");
        options.local = true;
        let mut config = SessionActorConfig::new(LaunchTarget::executable("codex"), AgentLaunch::Codex(options));
        config.mcp_servers.push(socket::server_entry(temp.path(), &[], temp.path(), &token));
        prepare(&state, &mut config).await.unwrap();
        assert!(config.local_mcp_bridge.is_none());
        assert!(config.local_team_lifetime.is_some());
        let entry = &config.mcp_servers[0];
        let addr = entry["url"].as_str().unwrap().strip_prefix("http://").unwrap().strip_suffix("/mcp").unwrap();
        for (auth, status) in [("bad", "403"), (entry["headers"][0]["value"].as_str().unwrap(), "200")] {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
            let headers = format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAuthorization: {auth}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",request.len());
            stream.write_all(format!("{headers}{request}").as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
            if status == "200" { assert!(response.contains("list_workers")); }
        }
    }
}
