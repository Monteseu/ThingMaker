//! A local agent for real: Claude Code's adapter, pointed at Ollama on this
//! computer, inside the sandbox, through the session actor. Runs only when
//! `THINGMAKER_REAL_LOCAL` names an installed model that uses tools.

use std::time::Duration;

use thingmaker_supervisor::{
    acp::LaunchTarget,
    agents::{
        PermissionStance,
        claude::launch::{ClaudeLaunchOptions, locate_adapter},
        local::{LocalKind, LocalTarget, sandbox},
    },
    supervisor::{AgentLaunch, Provider, SessionActor, SessionActorConfig, SessionEvent, TurnEffect, TurnPhase},
};

#[tokio::test]
async fn a_local_agent_writes_a_file_from_inside_the_sandbox() {
    let Ok(model) = std::env::var("THINGMAKER_REAL_LOCAL") else { return };
    let work = tempfile::tempdir().unwrap();
    // A real project's instructions, to check the start still fits:
    // `THINGMAKER_REAL_LOCAL_SEED=/path/to/project` copies its CLAUDE.md,
    // AGENTS.md and .claude folder in.
    if let Ok(seed) = std::env::var("THINGMAKER_REAL_LOCAL_SEED") {
        for name in ["CLAUDE.md", "AGENTS.md", ".claude"] {
            let from = std::path::Path::new(&seed).join(name);
            if from.exists() {
                assert!(std::process::Command::new("cp").arg("-R").arg(&from).arg(work.path()).status().unwrap().success());
            }
        }
    }
    let home_dir = tempfile::tempdir().unwrap();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let (node, script) = locate_adapter(&home, None, None).expect("claude-agent-acp is installed");
    let served = thingmaker_supervisor::agents::local::prepare_agent_model(&model, 16384).expect("Ollama made the window copy");
    let (program, prefix) = sandbox::wrap(&node, &[]).expect("the sandbox is available");
    let mut options = ClaudeLaunchOptions::new(work.path(), node.clone(), vec![script.to_string_lossy().into_owned()]);
    options.permission_mode = PermissionStance::AcceptEdits;
    options.local = Some(LocalTarget { kind: LocalKind::Agent, model: model.clone(), served, window: 16384, home: home_dir.path().to_path_buf() });
    let mut config = SessionActorConfig::new(LaunchTarget { executable: program, prefix_args: prefix }, AgentLaunch::Claude(options));
    config.environment = config.environment.with_program_dir(&node);
    config.timeouts.turn = Duration::from_secs(600);
    config.timeouts.control = Duration::from_secs(30);
    let actor = SessionActor::open(config).await.expect("attached");
    let snapshot = actor.snapshot().await.unwrap();
    assert_eq!(snapshot.provider, Provider::Local);
    eprintln!("options: {}", snapshot.config_options);
    let mut subscription = actor.subscribe();
    let started = std::time::Instant::now();
    actor.submit_text("r1", "Create a file named notes.txt in the current folder containing exactly the word hello. Use the Write tool.").await.expect("accepted");
    let phase = loop {
        let event = tokio::time::timeout(Duration::from_secs(600), subscription.recv()).await.expect("settled in time").expect("open");
        if !matches!(&event.payload, SessionEvent::Update(_)) {
            eprintln!("event: {:?}", event.payload);
        }
        if let SessionEvent::Turn(TurnEffect::Settled { phase, .. }) = &event.payload {
            break *phase;
        }
    };
    eprintln!("turn {phase:?} in {:?}", started.elapsed());
    let _ = actor.stop().await;
    assert_eq!(phase, TurnPhase::Succeeded);
    let written = std::fs::read_to_string(work.path().join("notes.txt")).expect("notes.txt was written");
    assert!(written.contains("hello"));
    assert!(home_dir.path().join("claude").is_dir(), "the agent used its own Claude Code folder, not ~/.claude");
}
