//! Native ACP routing, with a strict local peer and no Copilot credentials.

use std::{path::PathBuf, time::Duration};

use thingmaker_supervisor::{
    acp::{LaunchTarget, SessionUpdate, updates::ContentBlock},
    agents::{PermissionStance, copilot::CopilotLaunchOptions},
    delegation::socket::http_server_entry,
    supervisor::{AgentLaunch, Provider, SessionActor, SessionActorConfig, SessionEvent, TurnEffect, TurnPhase},
};

const PEER: &str = r#"
import json, sys, os
sys.stdin.reconfigure(encoding='utf-8')
assert '--acp' in sys.argv and '--no-auto-update' in sys.argv
assert '--trust-folder' not in sys.argv
assert os.path.samefile(sys.argv[sys.argv.index('-C') + 1], os.getcwd())
assert os.path.samefile(sys.argv[sys.argv.index('--add-dir') + 1], os.getcwd())
assert '--allow-all' not in sys.argv
assert os.environ['COPILOT_AUTO_UPDATE'] == 'false'
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    params = request.get('params', {})
    if method == 'initialize':
        assert params['clientInfo']['name']
        assert params['clientCapabilities']['_meta']['github.com/copilot']['interactive'] is False
        result = {'protocolVersion':1, 'agentInfo':{'name':'Copilot','version':'1.0.116'}, 'agentCapabilities':{'loadSession':True,'mcpCapabilities':{'http':True,'sse':True}}, 'authMethods':[]}
    elif method in ('session/new', 'session/load'):
        assert os.path.samefile(params['cwd'], os.getcwd())
        expected = next((arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--team-token=')), None)
        if expected:
            assert params['mcpServers'] == [{'name':'team','type':'http','url':'http://127.0.0.1:43210/mcp','headers':[{'name':'Authorization','value':'Bearer '+expected}]}]
            assert method == ('session/load' if '--resume-team' in sys.argv else 'session/new')
        else:
            assert params['mcpServers'] == []
        if '--signed-out' in sys.argv:
            send({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32000,'message':'Authentication required'}})
            continue
        result = {'sessionId':params.get('sessionId','copilot-test-session')}
    elif method == 'session/set_config_option':
        result = {}
    elif method == 'session/prompt':
        assert params['sessionId'] == 'copilot-test-session'
        if '--team-guidance' in sys.argv:
            texts = [block['text'] for block in params['prompt'] if block['type'] == 'text']
            if 'prompted' not in globals():
                assert texts[0] == 'Use the team tools for this group.'
            else:
                assert 'Use the team tools for this group.' not in texts
            prompted = True
        send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':params['sessionId'],'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'Copilot mock reply'}}}})
        result = {'stopReason':'end_turn'}
    else:
        if 'id' not in request:
            continue
        result = {}
    send({'jsonrpc':'2.0','id':request['id'],'result':result})
"#;

fn config(signed_out: bool, root: &std::path::Path) -> SessionActorConfig {
    let python = ["python3", "python"].into_iter().find(|program| {
        std::process::Command::new(program).arg("--version").output().is_ok_and(|output| output.status.success())
    }).expect("Python is required for the Copilot ACP peer test");
    let mut prefix_args = vec!["-u".into(), "-c".into(), PEER.into()];
    if signed_out { prefix_args.push("--signed-out".into()); }
    let mut options = CopilotLaunchOptions::new(root);
    options.permission_mode = PermissionStance::AcceptEdits;
    let mut config = SessionActorConfig::new(
        LaunchTarget { executable: PathBuf::from(python), prefix_args }, AgentLaunch::Copilot(options),
    );
    config.timeouts.control = Duration::from_secs(10);
    config.timeouts.turn = Duration::from_secs(10);
    config
}

#[tokio::test]
async fn standalone_session_uses_native_acp_and_completes_a_turn() {
    let root = tempfile::tempdir().unwrap();
    let actor = SessionActor::open(config(false, root.path())).await.expect("native ACP attached");
    let snapshot = actor.snapshot().await.unwrap();
    assert_eq!(snapshot.provider, Provider::Copilot);
    assert_eq!(snapshot.agent_session_id.as_deref(), Some("copilot-test-session"));
    assert_eq!(snapshot.capabilities.unwrap().agent_name, "Copilot");
    let mut events = actor.subscribe();
    actor.submit_text("copilot-prompt", "Reply once").await.unwrap();
    let text = tokio::time::timeout(Duration::from_secs(10), async {
        let mut text = String::new();
        while let Some(event) = events.recv().await {
            match &event.payload {
                SessionEvent::Update(SessionUpdate::AgentMessage(message)) => {
                    for block in &message.content {
                        if let ContentBlock::Text { text: chunk } = block { text.push_str(chunk); }
                    }
                }
                SessionEvent::Turn(TurnEffect::Settled { phase, .. }) => {
                    assert_eq!(*phase, TurnPhase::Succeeded);
                    return text;
                }
                _ => {}
            }
        }
        panic!("peer closed before settling");
    }).await.unwrap();
    assert_eq!(text, "Copilot mock reply");
    actor.stop().await.unwrap();
}

#[tokio::test]
async fn missing_credentials_fail_open_without_starting_a_turn() {
    let root = tempfile::tempdir().unwrap();
    let result = SessionActor::open(config(true, root.path())).await;
    assert!(result.is_err());
    assert!(result.err().unwrap().message.contains("Authentication required"));
}

async fn team_attachment(leads_team: bool, resume: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(false, root.path());
    let token = if leads_team { "leader-token" } else { "worker-token" };
    config.target.prefix_args.push(format!("--team-token={token}"));
    config.leads_team = leads_team;
    config.mcp_servers = vec![http_server_entry("http://127.0.0.1:43210/mcp", token)];
    let AgentLaunch::Copilot(options) = &mut config.launch else { unreachable!() };
    if resume {
        config.target.prefix_args.push("--resume-team".into());
        options.resume = Some("copilot-test-session".into());
    }
    if leads_team {
        config.target.prefix_args.push("--team-guidance".into());
        options.initial_prompt_prefix = Some("Use the team tools for this group.".into());
    }
    let actor = SessionActor::open(config).await.expect("team attachment received MCP through native ACP");
    let snapshot = actor.snapshot().await.unwrap();
    assert_eq!(snapshot.leads_team, leads_team);
    assert!(snapshot.capabilities.unwrap().supports_mcp);
    for id in ["first", "second"] {
        let mut events = actor.subscribe();
        actor.submit_text(id, "Reply once").await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = events.recv().await {
                if let SessionEvent::Turn(TurnEffect::Settled { phase, .. }) = event.payload {
                    assert_eq!(phase, TurnPhase::Succeeded);
                    return;
                }
            }
            panic!("peer closed before settling");
        }).await.unwrap();
    }
    actor.stop().await.unwrap();
}

#[tokio::test]
async fn a_copilot_leader_receives_its_http_team_on_new_and_load_and_guidance_once() {
    assert!(!Provider::Copilot.can_lead_goal());
    team_attachment(true, false).await;
    team_attachment(true, true).await;
}

#[tokio::test]
async fn a_copilot_worker_receives_its_own_http_team_on_new_and_load() {
    assert!(Provider::Copilot.worker_team_server());
    team_attachment(false, false).await;
    team_attachment(false, true).await;
}
