//! Opt-in production-path check: THINGMAKER_REAL_MEMBER_RESOURCES=1 cargo test
//! -p thingmaker-supervisor --test real_member_resources -- --nocapture
use std::{collections::HashMap, path::PathBuf, time::Duration};
use serde_json::json;
use thingmaker_supervisor::{
    acp::{LaunchTarget, SessionUpdate}, agents::{AgentLaunch, PermissionStance, Provider, codex::CodexLaunchOptions, claude::launch::{ClaudeLaunchOptions, locate_adapter}},
    integrations::member_resources::{self, ResourceSelection},
    supervisor::{SessionActor, SessionActorConfig, SessionEvent, TurnEffect, TurnPhase},
};

#[tokio::test]
async fn codex_worker_uses_shared_skill_resources_and_only_selected_mcp() {
    real_worker(Provider::Codex).await;
}

#[tokio::test]
async fn claude_worker_uses_declared_skill_dependency_and_only_selected_mcp() {
    real_worker(Provider::Claude).await;
}

async fn real_worker(provider: Provider) {
    if std::env::var("THINGMAKER_REAL_MEMBER_RESOURCES").as_deref() != Ok("1") { return }
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let skills = root.join(".agents/skills/shared-art-integration");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(skills.join("SKILL.md"), "---\nname: shared-art-integration\ndescription: Read the palette for this integration test.\n---\nRead palette.txt in this skill directory and report its content.\n").unwrap();
    std::fs::create_dir_all(skills.join("agents")).unwrap();
    std::fs::write(skills.join("agents/openai.yaml"), "dependencies:\n  tools:\n    - type: mcp\n      value: selected\n").unwrap();
    std::fs::write(skills.join("palette.txt"), "PALETTE-FROM-SHARED-SKILL-73921").unwrap();
    let log = root.join("mcp.log");
    let server = root.join("server.py");
    std::fs::write(&server, r#"
import json,sys
log,label=sys.argv[1:]
with open(log,'a') as f: f.write(label+':started\n')
for line in sys.stdin:
 m=json.loads(line)
 if 'id' not in m: continue
 result={}
 if m.get('method')=='initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':label,'version':'1'}}
 elif m.get('method')=='tools/list': result={'tools':[{'name':'stamp','description':'Return a test marker','inputSchema':{'type':'object','properties':{}}}]}
 elif m.get('method')=='tools/call':
  with open(log,'a') as f: f.write(label+':called\n')
  result={'content':[{'type':'text','text':'STAMP-PASS'}]}
 print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)
"#).unwrap();
    let python = ["python3", "python"].into_iter().find(|program| std::process::Command::new(program).arg("--version").output().is_ok_and(|output| output.status.success())).expect("Python is installed");
    let server_args = |label: &str| vec![server.to_string_lossy().into_owned(), log.to_string_lossy().into_owned(), label.to_string()];
    std::fs::write(root.join(".mcp.json"), json!({"mcpServers":{"selected":{"command":python,"args":server_args("selected")},"excluded":{"command":python,"args":server_args("excluded")}}}).to_string()).unwrap();
    let found = member_resources::catalog(&root.join("fake-home"), &root.join("fake-home/.claude"), Some(&root)).unwrap().for_provider(provider);
    let selected_skill = "skill:agents-project:shared-art-integration";
    let dependency = &found.entries.iter().find(|entry| entry.id == selected_skill).unwrap().mcp_dependencies[0];
    assert!(dependency.required);
    assert_eq!(dependency.mcp_ids, ["mcp:project:selected"]);
    let mut selection = ResourceSelection { skills:vec![selected_skill.into()],mcps:vec![] };
    assert!(member_resources::resolve(&found, &selection, &HashMap::new()).is_err(), "required MCP is not silently auto-enabled");
    selection.mcps = dependency.mcp_ids.clone();
    let resolved = member_resources::resolve(&found, &selection, &HashMap::new()).unwrap();
    let (target, mut launch) = if provider == Provider::Codex {
        let program = PathBuf::from(std::env::var("THINGMAKER_REAL_CODEX_PROGRAM").unwrap_or_else(|_| "codex".into()));
        let mut options = CodexLaunchOptions::new(&root, program.clone());
        options.model = Some("gpt-6-luna".into());
        options.effort = Some("low".into());
        options.permission_mode = PermissionStance::AcceptEdits;
        let target = LaunchTarget { executable:program, prefix_args:vec![
            "-c".into(),format!("mcp_servers.excluded.command={}", json!(python)),
            "-c".into(),format!("mcp_servers.excluded.args={}", json!(server_args("excluded"))),
        ] };
        (target, AgentLaunch::Codex(options))
    } else {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let (node, script) = locate_adapter(&home, None, std::env::var("THINGMAKER_CLAUDE_ACP").ok()).expect("Claude adapter installed");
        let mut options = ClaudeLaunchOptions::new(&root, node.clone(), vec![script.to_string_lossy().into_owned()]);
        options.model = Some("haiku".into());
        options.effort = Some("low".into());
        options.permission_mode = PermissionStance::AcceptEdits;
        (LaunchTarget::executable(node), AgentLaunch::Claude(options))
    };
    let files = member_resources::apply_to_launch(resolved, &mut launch).unwrap();
    let mut config = SessionActorConfig::new(target, launch);
    config.resource_files = files;
    let actor = SessionActor::open(config).await.expect("restricted worker opened through the production bridge");
    let mut events = actor.subscribe();
    actor.submit_text("probe", "Use the shared-art-integration skill in your available skills catalog, read its palette.txt, then call the selected server's stamp MCP tool exactly once. Report the palette and stamp results. Do not read unrelated files.").await.unwrap();
    let mut tool_output = String::new();
    let phase = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match &events.recv().await.expect("event stream").payload {
                SessionEvent::Update(SessionUpdate::ToolCall(patch) | SessionUpdate::ToolCallUpdate(patch)) => tool_output.push_str(&serde_json::to_string(patch).unwrap()),
                SessionEvent::Turn(TurnEffect::Settled { phase, .. }) => break *phase,
                _ => {}
            }
        }
    }).await;
    let _ = actor.stop().await;
    assert_eq!(phase.expect("probe settled"), TurnPhase::Succeeded);
    assert!(tool_output.contains("PALETTE-FROM-SHARED-SKILL-73921"), "the supporting file was actually read");
    let log = std::fs::read_to_string(log).unwrap();
    assert!(log.contains("selected:called"), "selected MCP was invoked");
    assert!(!log.contains("excluded:started"), "inherited excluded MCP never started");
}
