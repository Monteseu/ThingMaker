//! Delegation across the two real providers, on the user's own plans: a
//! Claude orchestrator hands a task to a Codex worker, and a Codex
//! orchestrator hands one to a Claude worker. Each spends a few small turns,
//! so the test only runs when asked:
//!
//! ```text
//! THINGMAKER_REAL_DELEGATION=1 cargo test -p thingmaker-supervisor --test real_delegation -- --nocapture --test-threads=1
//! ```
//!
//! What it proves that the mocks cannot: each provider starts the ThingMaker MCP
//! server from the session's own configuration (ACP `mcpServers` for Claude,
//! a `mcp_servers.thingmaker` override for Codex), the model finds and calls the
//! tools, and a worker on the other provider does the work.

use std::{path::PathBuf, sync::Arc, time::Duration};

use thingmaker_supervisor::{
    DesktopError,
    acp::{LaunchTarget, SessionUpdate, updates::ContentBlock},
    agents::{PermissionStance, Provider, claude::launch::{ClaudeLaunchOptions, locate_adapter}, codex::{CodexLaunchOptions, launch::MULTI_AGENT_FEATURE}},
    delegation::{Combo, Delegation, JobStatus, WorkerLauncher, WorkerSlot, WorkerSpec, jobs::BoxFuture, orchestrator_guidance, socket::{DelegationSocket, server_entry}},
    supervisor::{AgentLaunch, SessionActor, SessionActorConfig, SessionEvent, TurnEffect},
};

fn codex() -> PathBuf {
    std::env::var_os("THINGMAKER_CODEX").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex"))
}

fn launch(provider: Provider, root: &std::path::Path, model: Option<String>, effort: Option<String>) -> (LaunchTarget, AgentLaunch) {
    match provider {
        Provider::Claude => {
            let home = PathBuf::from(std::env::var("HOME").unwrap());
            let (node, script) = locate_adapter(&home, None, std::env::var("THINGMAKER_CLAUDE_ACP").ok()).expect("claude-agent-acp is installed");
            let mut options = ClaudeLaunchOptions::new(root, node.clone(), vec![script.to_string_lossy().into_owned()]);
            options.model = model;
            options.effort = effort;
            options.permission_mode = PermissionStance::AcceptEdits;
            (LaunchTarget::executable(node), AgentLaunch::Claude(options))
        }
        Provider::Codex => {
            let mut options = CodexLaunchOptions::new(root, codex());
            options.model = model;
            options.effort = effort;
            options.permission_mode = PermissionStance::AcceptEdits;
            (LaunchTarget::executable(codex()), AgentLaunch::Codex(options))
        }
        Provider::Gemini => {
            let agy = PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/agy");
            let mut options = thingmaker_supervisor::agents::gemini::GeminiLaunchOptions::new(root, agy.clone());
            options.model = model;
            options.effort = effort;
            options.permission_mode = PermissionStance::ReadOnly;
            (LaunchTarget::executable(agy), AgentLaunch::Gemini(options))
        }
        Provider::Copilot => unimplemented!("Copilot standalone is exercised in copilot_peer.rs"),
        Provider::Local => unimplemented!("local models are exercised in local_chat.rs"),
    }
}

struct RealLauncher;

impl WorkerLauncher for RealLauncher {
    fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
        let (target, launch) = launch(spec.slot.provider, &spec.root, spec.slot.model.clone(), spec.slot.effort.clone());
        Box::pin(async move { SessionActor::open(SessionActorConfig::new(target, launch)).await })
    }
    fn released(&self, _worker_handle: &str) {}
}

async fn run(orchestrator: Provider, worker: WorkerSlot) {
    let scratch = tempfile::tempdir().unwrap();
    let delegation = Delegation::new(Arc::new(RealLauncher), |job| eprintln!("job {} {:?} {:?}", job.id, job.status, job.error));
    let socket = DelegationSocket::bind(delegation.clone(), scratch.path().join("mcp.sock")).unwrap();
    let combo = Combo { workers: vec![worker.clone()], native_subagents: false, ..Default::default() };

    let (model, effort) = match orchestrator {
        Provider::Claude => (Some("sonnet".to_string()), Some("low".to_string())),
        Provider::Codex => (Some("gpt-6-luna".to_string()), Some("low".to_string())),
        Provider::Gemini | Provider::Copilot | Provider::Local => unreachable!("not orchestrated in this test"),
    };
    let (target, mut agent) = launch(orchestrator, scratch.path(), model, effort);
    let key = agent.session_id();
    let token = delegation.register(&key, orchestrator, scratch.path().to_path_buf(), combo);
    let mut config = SessionActorConfig::new(target, agent.clone());
    config.mcp_servers = vec![server_entry(&PathBuf::from(env!("CARGO_BIN_EXE_thingmaker-mcp")), &[], socket.path(), &token)];
    match &mut agent {
        AgentLaunch::Claude(options) => {
            options.disallowed_tools = vec!["Agent".into(), "Task".into()];
            options.append_system_prompt = Some(orchestrator_guidance(true));
            config.environment = config.environment.clone().with_set("MCP_TOOL_TIMEOUT", "1800000");
        }
        AgentLaunch::Codex(options) => {
            options.config.insert(MULTI_AGENT_FEATURE.into(), serde_json::Value::Bool(false));
            options.developer_instructions = Some(orchestrator_guidance(true));
        }
        AgentLaunch::Gemini(_) | AgentLaunch::Copilot(_) => unreachable!("Gemini cannot orchestrate"),
    }
    config.launch = agent;
    let actor = SessionActor::open(config).await.expect("orchestrator attached");
    let mut subscription = actor.subscribe();
    let prompt = format!(
        "Test of the team tools. Call list_workers, then delegate to the worker named {} the task: \"Reply with exactly the word PONG and nothing else. Do not run any commands.\" \
Then call await_jobs for that job and reply with the worker's report verbatim, nothing else.",
        worker.name
    );
    actor.submit_text("r1", &prompt).await.expect("submitted");
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(420);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(event)) = tokio::time::timeout(remaining, subscription.recv()).await else { panic!("no settle; text {text:?}") };
        match &event.payload {
            SessionEvent::Update(SessionUpdate::AgentMessage(message)) => {
                let chunk: String = message.content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                if message.replace { text = chunk } else { text.push_str(&chunk) }
            }
            SessionEvent::Update(SessionUpdate::ToolCall(patch)) | SessionEvent::Update(SessionUpdate::ToolCallUpdate(patch)) => {
                if let Some(title) = &patch.title {
                    eprintln!("tool {:?} {title}", patch.status);
                }
            }
            SessionEvent::Diagnostic { text } | SessionEvent::AgentOutput { text } => eprintln!("diagnostic: {}", text.chars().take(300).collect::<String>()),
            SessionEvent::Turn(TurnEffect::Settled { phase, error, .. }) => {
                eprintln!("settled {phase:?} {error:?}");
                break;
            }
            _ => {}
        }
    }
    let jobs = delegation.jobs(&key);
    eprintln!("orchestrator said {text:?}\njobs {jobs:#?}");
    assert_eq!(jobs.len(), 1, "exactly one delegation");
    assert_eq!(jobs[0].status, JobStatus::Succeeded);
    assert_eq!(jobs[0].provider, worker.provider);
    assert!(jobs[0].result.as_deref().unwrap_or_default().to_ascii_uppercase().contains("PONG"));
    assert!(text.to_ascii_uppercase().contains("PONG"), "the orchestrator relayed the report");
    delegation.release(&key).await;
    let _ = actor.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claude_orchestrator_delegates_to_a_codex_worker() {
    if std::env::var("THINGMAKER_REAL_DELEGATION").as_deref() != Ok("1") {
        eprintln!("set THINGMAKER_REAL_DELEGATION=1 to spend real turns; skipping");
        return;
    }
    let worker = WorkerSlot { name: "luna".into(), provider: Provider::Codex, model: Some("gpt-6-luna".into()), effort: Some("low".into()), capabilities: vec!["fast".into()], note: None, prompt: None, resources: None };
    run(Provider::Claude, worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_codex_orchestrator_delegates_to_a_claude_worker() {
    if std::env::var("THINGMAKER_REAL_DELEGATION").as_deref() != Ok("1") {
        eprintln!("set THINGMAKER_REAL_DELEGATION=1 to spend real turns; skipping");
        return;
    }
    let worker = WorkerSlot { name: "sonnet".into(), provider: Provider::Claude, model: Some("sonnet".into()), effort: Some("low".into()), capabilities: vec!["code".into()], note: None, prompt: None, resources: None };
    run(Provider::Codex, worker).await;
}


/// Two real workers use their own MCP tokens. Only the recipient sees the
/// random word, in its first turn before the idle reaper closes its process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "uses two real Codex agents"]
async fn chat_reopens_the_original_conversation() {
    use thingmaker_supervisor::delegation::jobs::DelegateArgs;
    struct ChatLauncher(PathBuf);
    impl WorkerLauncher for ChatLauncher {
        fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
            let (target, mut agent) = launch(Provider::Codex, &spec.root, spec.slot.model.clone(), Some("low".into()));
            let AgentLaunch::Codex(options) = &mut agent else { unreachable!() };
            options.permission_mode = PermissionStance::AcceptEdits;
            options.resources = Some(Default::default());
            options.resume = spec.resume;
            let mut config = SessionActorConfig::new(target, agent);
            config.mcp_servers = vec![server_entry(std::path::Path::new(env!("CARGO_BIN_EXE_thingmaker-mcp")), &[], &self.0, spec.mcp_token.as_deref().unwrap())];
            Box::pin(async move { SessionActor::open(config).await })
        }
        fn released(&self, _: &str) {}
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sock");
    let service = Delegation::new(Arc::new(ChatLauncher(path.clone())), |job| eprintln!("{} {:?} {:?}", job.id, job.status, job.error));
    let _socket = DelegationSocket::bind(service.clone(), path).unwrap();
    let slot = |name: &str| WorkerSlot { name: name.into(), provider: Provider::Codex, model: None, effort: Some("low".into()), capabilities: vec![], note: None, prompt: None, resources: None };
    let team = "real-chat";
    service.register(team, Provider::Codex, dir.path().into(), Combo { workers: vec![slot("Keeper"),slot("Asker")], native_subagents: false, ..Default::default() });
    let word = uuid::Uuid::new_v4().simple().to_string();
    let keeper = service.delegate(team, DelegateArgs { worker: Some("Keeper".into()), task:format!("Remember this private code in this conversation only: {word}. Reply READY now. Never write a file or use shell. Later, if another team member asks for the code, reply with it through chat_send."), ..Default::default() }).unwrap();
    let done = service.await_jobs(team, &[keeper.id.clone()], Duration::from_secs(100), false).await.unwrap();
    assert_eq!(done[0].status, JobStatus::Succeeded, "{done:?}");
    let original = done[0].agent_session_id.clone().unwrap();
    service.set_worker_idle(Duration::ZERO);
    service.reap_idle().await;
    service.set_worker_idle(Duration::from_secs(300));
    assert!(service.job(team,&keeper.id).unwrap().worker_closed);
    let asker = service.delegate(team, DelegateArgs { worker: Some("Asker".into()), task:format!("Use the team chat_send tool now. Address: {}. Ask for the private code remembered earlier. Do not inspect files or use shell. After sending, finish this turn. If you later receive the reply just finish; do not send an acknowledgement.",keeper.id), ..Default::default() }).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        let history = service.chat_history(team);
        assert!(history.iter().all(|m| m.error.is_none()), "delivery error: {:?}", history.iter().filter_map(|m| m.error.as_ref()).collect::<Vec<_>>());
        if history.iter().any(|m| m.delivered && m.from == keeper.id && m.to == asker.id && m.reply_to == Some(1) && m.text.contains(&word)) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no contextual reply; messages: {:?}, jobs: {:?}", history.iter().map(|m| (&m.from,&m.to,m.delivered)).collect::<Vec<_>>(),service.jobs(team).iter().map(|j| (&j.worker,j.status,&j.error,&j.result)).collect::<Vec<_>>());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(service.jobs(team).iter().any(|j| j.continues.as_ref()==Some(&keeper.id) && j.agent_session_id.as_ref()==Some(&original)));
    // By id: the jobs the messages started are not among those awaited by default.
    let ids: Vec<String> = service.jobs(team).into_iter().map(|job| job.id).collect();
    let jobs = service.await_jobs(team, &ids, Duration::from_secs(60), false).await.unwrap();
    assert!(jobs.len() >= 3 && jobs.iter().all(|j| j.status == JobStatus::Succeeded), "{jobs:?}");
    assert!(jobs.iter().any(|j| j.chat), "the reply was a job a message started: {jobs:?}");
    service.release(team).await;
}
