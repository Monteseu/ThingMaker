//! A Local Models chat through the real session actor: ThingMaker's own chat
//! agent (`thingmaker-local-chat`) against a stand-in Ollama on a local port.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    time::Duration,
};

use thingmaker_supervisor::{
    acp::LaunchTarget,
    agents::{
        PermissionStance,
        claude::launch::ClaudeLaunchOptions,
        local::{LocalKind, LocalTarget},
    },
    supervisor::{AgentLaunch, Provider, SessionActor, SessionActorConfig, SessionEvent, TurnEffect, TurnPhase},
};

/// Answers `/api/chat` with a streamed reply, and records each request body.
fn fake_ollama() -> (String, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let unloads = Arc::new(Mutex::new(Vec::new()));
    let unloaded = Arc::clone(&unloads);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap_or(0);
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
            let reply = if path == "/api/chat" {
                seen.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
                let lines = [
                    r#"{"message":{"role":"assistant","content":"Abyss "},"done":false}"#,
                    r#"{"message":{"role":"assistant","content":"Antics"},"done":false}"#,
                    r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":12,"eval_count":3,"total_duration":5000000}"#,
                ];
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{}\n", lines.join("\n"))
            } else if path == "/api/generate" {
                unloaded.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
                let body = r#"{"done":true,"done_reason":"unload"}"#;
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            } else if path == "/api/tags" {
                let body = r#"{"models":[]}"#;
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            } else {
                let body = r#"{"error":"not found"}"#;
                format!("HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (address, bodies, unloads)
}

fn config(address: &str, home: &std::path::Path, resume: Option<String>) -> SessionActorConfig {
    let program = std::path::PathBuf::from(env!("CARGO_BIN_EXE_thingmaker-local-chat"));
    let mut options = ClaudeLaunchOptions::new(home, program.clone(), Vec::new());
    options.permission_mode = PermissionStance::AcceptEdits;
    options.resume = resume;
    options.local = Some(LocalTarget { kind: LocalKind::Chat, model: "qwen3:8b".into(), served: "qwen3:8b".into(), window: 8192, home: home.to_path_buf() });
    let mut config = SessionActorConfig::new(LaunchTarget::executable(program), AgentLaunch::Claude(options));
    config.environment = config.environment.with_set("THINGMAKER_OLLAMA_ADDRESS", address);
    config.timeouts.turn = Duration::from_secs(10);
    config.timeouts.control = Duration::from_secs(5);
    config.timeouts.shutdown = Duration::from_secs(5);
    config
}

async fn settle(subscription: &mut thingmaker_supervisor::supervisor::EventSubscription) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, subscription.recv()).await.unwrap_or_else(|_| panic!("timed out; saw {seen:#?}")).expect("stream open");
        let done = matches!(&event.payload, SessionEvent::Turn(TurnEffect::Settled { .. }));
        seen.push(event.payload.clone());
        if done {
            return seen;
        }
    }
}

#[tokio::test]
async fn a_local_chat_answers_through_the_session_actor_and_resumes_with_its_history() {
    let (address, bodies, unloads) = fake_ollama();
    let home = tempfile::tempdir().unwrap();

    let config = config(&address, home.path(), None);
    assert_eq!(config.launch.provider(), Provider::Local);
    let actor = SessionActor::open(config).await.expect("attached");
    let mut subscription = actor.subscribe();
    actor.submit_text("r1", "Name a deep-sea theme").await.expect("accepted");
    let events = settle(&mut subscription).await;
    assert!(
        events.iter().any(|event| matches!(event, SessionEvent::Turn(TurnEffect::Settled { phase, .. }) if *phase == TurnPhase::Succeeded)),
        "the turn succeeded; saw {events:#?}"
    );
    let snapshot = actor.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.provider, Provider::Local);
    let session_id = snapshot.agent_session_id.expect("the chat agent named the session");
    assert!(unloads.lock().unwrap().is_empty(), "keep the model loaded while the session is open");
    actor.stop().await.expect("stopped");
    assert_eq!(*unloads.lock().unwrap(), vec![serde_json::json!({"model":"qwen3:8b", "keep_alive":0, "stream":false})]);
    assert!(bodies.lock().unwrap()[0].contains("Name a deep-sea theme"));

    // ThingMaker restarts: the chat resumes, and the model sees the history.
    let actor = SessionActor::open(config_with(&address, home.path(), session_id)).await.expect("resumed");
    let mut subscription = actor.subscribe();
    actor.submit_text("r2", "Another one").await.expect("accepted");
    settle(&mut subscription).await;
    actor.stop().await.expect("stopped after resume");
    assert_eq!(unloads.lock().unwrap().len(), 2, "each stopped attachment unloads its model");
    let second = bodies.lock().unwrap()[1].clone();
    assert!(second.contains("Name a deep-sea theme") && second.contains("Abyss Antics") && second.contains("Another one"), "the second request carried the whole conversation: {second}");
}

fn config_with(address: &str, home: &std::path::Path, session_id: String) -> SessionActorConfig {
    config(address, home, Some(session_id))
}
