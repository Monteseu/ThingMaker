//! Teams and jobs (docs/research/multi-provider-viability.md §2.2).
//!
//! Every session the user opens is an orchestrator: it gets the ThingMaker MCP
//! server, and its team (the combo) decides who `delegate` hands work to.
//! Workers are ordinary attachments the host opens here, on whichever
//! provider the combo names, so the user can open one and watch it like any
//! other session. Job changes reach the renderer as `thingmaker://job` events.

use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};
use thingmaker_supervisor::{
    DesktopError, SessionHandle,
    acp::SessionId,
    agents::Provider,
    delegation::{
        Combo, Delegation, JobView, WorkerLauncher, WorkerSpec,
        jobs::{
            BoxFuture, RetryPolicy,
            chat::{ChatEvent, ChatView, Receipt},
        },
        orchestrator_guidance,
        socket::{DelegationSocket, RELAY_ARG, server_entry, socket_path},
    },
    storage::{settings::GLOBAL_SCOPE, workspaces::SessionOrigin},
    supervisor::{AgentLaunch, SessionActor, SessionActorConfig, SessionEvent},
};

use super::{CommandResult, storage_error};
use crate::state::AppState;

pub const JOB_EVENT: &str = "thingmaker://job";
/// A session's agent chat changed; the payload is `{ handle }`, and the
/// renderer asks for the chat again. Not passed on to the phone, which reads
/// the chat when it shows it.
pub const TEAM_CHAT_EVENT: &str = "thingmaker://team-chat";
/// Settings key for the receipts of the agent messages written into one
/// conversation, scope `conversation:<the agent's id for it>`. They are what
/// lets the transcript show such a prompt as its sender's rather than the
/// person's, and they outlive the attachment: the provider replays the
/// prompt when the session is reopened, and the receipt has to be there.
pub const CHAT_RECEIPTS_KEY: &str = "teamChatReceipts";
/// Receipts one conversation keeps, newest last. A session's chat holds
/// sixty messages, so this is many reopenings of the same conversation.
const CHAT_RECEIPTS_KEPT: usize = 1000;

fn receipts_scope(agent_session_id: &str) -> String {
    format!("conversation:{agent_session_id}")
}

/// Keeps a receipt with the conversation of the attachment it is for.
fn keep_receipt(state: &AppState, recipient: &str, receipt: Receipt) {
    let scope = receipts_scope(&state.agent_session_id(recipient));
    let kept = state.with_storage(|s| {
        let mut kept: Vec<Receipt> = s.setting_get(CHAT_RECEIPTS_KEY, &scope)?.unwrap_or_default();
        if kept.iter().any(|known| known.token == receipt.token) {
            return Ok(());
        }
        kept.push(receipt);
        let extra = kept.len().saturating_sub(CHAT_RECEIPTS_KEPT);
        kept.drain(..extra);
        s.setting_set(CHAT_RECEIPTS_KEY, &scope, &kept)
    });
    if let Err(error) = kept {
        tracing::warn!(%error, "an agent message's receipt was not kept; its transcript will show it as a prompt");
    }
}

/// The receipts kept for an attachment's conversation.
fn receipts(state: &AppState, handle: &str) -> Vec<Receipt> {
    let scope = receipts_scope(&state.agent_session_id(handle));
    state.with_storage(|s| s.setting_get(CHAT_RECEIPTS_KEY, &scope)).ok().flatten().unwrap_or_default()
}
/// Settings key for a team: scope `global` for the default a new session
/// starts with, `session:<row id>` for one session's own.
pub const COMBO_KEY: &str = "combo";
/// Settings key (scope `global`) for what a job does after a temporary limit.
pub const RETRY_KEY: &str = "delegationRetry";
/// How long Claude Code waits on one MCP tool call, in milliseconds. It has
/// to outlast `await_jobs`.
const CLAUDE_MCP_TOOL_TIMEOUT_MS: &str = "1800000";

pub use thingmaker_supervisor::storage::workspaces::GEMINI_NATIVE as GEMINI_NATIVE_KEY;

pub fn session_scope(row_id: &str) -> String {
    format!("session:{row_id}")
}

/// Whether a Gemini session about to open runs on native `agy acp`, the only
/// mode that takes the session's own `team` server. A new one does when it
/// starts with a team to lead; a reopened one does when it began that way.
/// Every other Gemini session is the plain stream-json session it always was.
pub fn gemini_native(state: &AppState, resumed_row: Option<&str>, combo: &Combo) -> bool {
    match resumed_row {
        Some(row) => state.with_storage(|s| s.setting_get::<bool>(GEMINI_NATIVE_KEY, row)).ok().flatten().unwrap_or(false),
        None => !combo.workers.is_empty(),
    }
}

/// Starts the service and its socket; called once from `setup`.
pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let launcher = Arc::new(HostLauncher { app: app.clone() });
    let emitter = app.clone();
    let delegation = Delegation::new(launcher, move |job: &JobView| {
        let _ = emitter.emit(JOB_EVENT, job);
        // Big Thing follows the jobs it gave out, and the tasks they are named after.
        if let Some(engine) = emitter.try_state::<AppState>().and_then(|state| state.engine.get().cloned()) {
            engine.note_job(job);
        }
    });
    delegation.add_extension(Arc::new(super::continuity::ContinuityTools { app: app.clone() }));
    let chat = app.clone();
    delegation.on_chat(move |event| match event {
        ChatEvent::Changed { session } => {
            let _ = chat.emit(TEAM_CHAT_EVENT, serde_json::json!({ "handle": session }));
        }
        // Kept before the prompt is written, so the transcript finds it.
        ChatEvent::Receiving { recipient, receipt } => {
            if let Some(state) = chat.try_state::<AppState>() {
                keep_receipt(&state, &recipient, receipt);
            }
            let _ = chat.emit(TEAM_CHAT_EVENT, serde_json::json!({ "handle": recipient }));
        }
        _ => {}
    });
    let path = socket_path(&state.data_dir);
    let socket = tauri::async_runtime::block_on(async { DelegationSocket::bind(delegation.clone(), path) }).map_err(|error| format!("could not open the ThingMaker MCP socket: {error}"))?;
    tracing::info!(path = %socket.path().display(), "ThingMaker MCP socket listening");
    if let Ok(Some(policy)) = state.with_storage(|s| s.setting_get::<RetryPolicy>(RETRY_KEY, GLOBAL_SCOPE)) {
        delegation.set_retry_policy(policy);
    }
    let _ = state.delegation.set(delegation);
    let _ = state.delegation_socket.set(socket);
    Ok(())
}

/// The program an agent starts as its ThingMaker MCP server: this executable,
/// which relays instead of opening a window when given [`RELAY_ARG`].
fn relay_program() -> Option<PathBuf> {
    std::env::var_os("THINGMAKER_MCP_PROGRAM").map(PathBuf::from).or_else(|| std::env::current_exe().ok())
}

/// The team a session starts with: the one asked for, else the one it had
/// last time, else none. New sessions start without a team; one is chosen
/// in the composer.
pub fn starting_combo(state: &AppState, requested: Option<Combo>, row_id: Option<&str>) -> Combo {
    if let Some(combo) = requested {
        return combo.normalized();
    }
    let stored = row_id.and_then(|id| state.with_storage(|s| s.setting_get::<Combo>(COMBO_KEY, &session_scope(id))).ok().flatten());
    stored.unwrap_or_default().normalized()
}

/// Makes the attachment about to open an orchestrator: registers it, hands
/// it the ThingMaker MCP server, and tells it about its team. Without the
/// service (it failed to start) the session opens as a plain one. Says
/// whether the session leads a team.
///
/// `gemini_native` is [`gemini_native`]'s answer; other providers ignore it.
pub fn prepare_orchestrator(state: &AppState, key: &str, root: PathBuf, combo: &Combo, launch: &mut AgentLaunch, config: &mut SessionActorConfig, gemini_native: bool) -> bool {
    // A provider that cannot take the session's own MCP server cannot lead
    // a team: it opens as a plain session.
    if !launch.provider().can_orchestrate() {
        return false;
    }
    if let AgentLaunch::Gemini(options) = launch {
        if !gemini_native {
            return false;
        }
        // The transport is the conversation's, with or without the service.
        options.native_acp = true;
    }
    let (Some(delegation), Some(socket), Some(program)) = (state.delegation.get(), state.delegation_socket.get(), relay_program()) else {
        return false;
    };
    let token = delegation.register(key, launch.provider(), root, combo.clone());
    config.mcp_servers = vec![server_entry(&program, &[RELAY_ARG.to_string()], socket.path(), &token)];
    config.leads_team = true;
    let withheld = combo.withholds_native_subagents();
    let guidance = orchestrator_guidance(withheld);
    match launch {
        AgentLaunch::Claude(options) => {
            if withheld {
                options.disallowed_tools = vec!["Agent".into(), "Task".into()];
            }
            options.append_system_prompt = Some(guidance);
            config.environment = config.environment.clone().with_set("MCP_TOOL_TIMEOUT", CLAUDE_MCP_TOOL_TIMEOUT_MS);
        }
        AgentLaunch::Codex(options) => {
            if withheld {
                options.config.insert(thingmaker_supervisor::agents::codex::launch::MULTI_AGENT_FEATURE.into(), serde_json::Value::Bool(false));
            }
            options.developer_instructions = Some(guidance);
        }
        AgentLaunch::Copilot(options) => {
            let mut prompt = orchestrator_guidance(false);
            if withheld {
                prompt.push_str(" For delegated work, use only the selected team workers through the team tools; do not start native subagents.");
            }
            options.initial_prompt_prefix = Some(prompt);
        }
        AgentLaunch::Gemini(options) => {
            // Native ACP's system-prompt extension has no established payload
            // contract here. Use an ordinary prompt block instead of guessing.
            // Do not claim native subagent tools have been disabled.
            let mut prompt = orchestrator_guidance(false);
            if withheld {
                prompt.push_str(" For delegated work, use only the selected team workers through the team tools; do not start native subagents.");
            }
            options.initial_prompt_prefix = Some(prompt);
        }
    }
    true
}

/// After the orchestrator is open: its quota reports feed the router.
pub fn watch_orchestrator(state: &AppState, actor: &SessionActor) {
    let Some(delegation) = state.delegation.get().cloned() else { return };
    delegation.attach_chat_lead(actor);
    let mut subscription = actor.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = subscription.recv().await {
            match &event.payload {
                SessionEvent::Quota(snapshot) => delegation.note_quota(snapshot.clone()),
                SessionEvent::Exited(_) => break,
                _ => {}
            }
        }
    });
}

/// The person wrote in this session (a prompt, a queued one, a steer): the
/// agents of its team may start work for each other again.
pub fn note_person(state: &AppState, handle: &str) {
    if let Some(delegation) = state.delegation.get() {
        delegation.note_person(handle);
    }
}

/// Forgets an orchestrator and stops its workers.
pub async fn release(state: &AppState, key: &str) {
    if let Some(delegation) = state.delegation.get() {
        delegation.release(key).await;
    }
}

/// The account a worker of `orchestrator` runs on: the leader's own when the
/// person pinned the leader to one and the worker is on the same provider,
/// since that is the account they chose to spend; the provider's active
/// account otherwise.
fn worker_account(state: &AppState, orchestrator: &str, provider: Provider) -> String {
    state.pinned_account(orchestrator).filter(|(pinned, _)| *pinned == provider).map(|(_, account)| account).unwrap_or_else(|| state.active_account(provider))
}

/// What a worker opened with: its account, its workspace and its resolved
/// resources. A worker whose revision changed is not reused.
fn revision_of(account: &str, root: &std::path::Path, resolved: Option<&thingmaker_supervisor::integrations::member_resources::ResolvedResources>) -> String {
    serde_json::to_string(&serde_json::json!([account, root, resolved.map(|value| &value.revision)])).unwrap_or_default()
}

struct HostLauncher {
    app: AppHandle,
}

impl WorkerLauncher for HostLauncher {
    fn revision(&self, orchestrator: &str, root: &std::path::Path, slot: &thingmaker_supervisor::delegation::WorkerSlot) -> CommandResult<Option<String>> {
        let state = self.app.state::<AppState>();
        let resolved = super::member_resources::resolve(&state, root, slot)?;
        Ok(Some(revision_of(&worker_account(&state, orchestrator, slot.provider), root, resolved.as_ref())))
    }
    fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
        let launched = self.launch_with_revision(spec);
        Box::pin(async move { launched.await.map(|(actor, _)| actor) })
    }
    fn launch_with_revision(&self, spec: WorkerSpec) -> BoxFuture<Result<(SessionActor, Option<String>), DesktopError>> {
        let app = self.app.clone();
        Box::pin(async move {
            let state = app.state::<AppState>();
            let root = spec.root.to_string_lossy().into_owned();
            let record = state
                .with_storage(|s| s.workspace_by_root(&root))
                .map_err(storage_error)?
                .ok_or_else(|| DesktopError::not_ready("the orchestrator's workspace is gone"))?;
            let record = super::workspace::ensure_trusted(&state, record)?;
            let reserved = SessionId::reserve();
            let account = worker_account(&state, &spec.orchestrator, spec.slot.provider);
            let (target, mut launch) = super::session::build_launch_for_account(&state, &record, spec.slot.provider, spec.slot.model.clone(), spec.slot.effort.clone(), spec.resume.clone(), reserved.clone(), &account)?;
            // Resolved once: the revision recorded is the one this launch used.
            let resolved = super::member_resources::resolve(&state, &spec.root, &spec.slot)?;
            let revision = revision_of(&account, &spec.root, resolved.as_ref());
            if spec.resume.is_some() && spec.resume_revision.as_ref() != Some(&revision) {
                return Err(DesktopError::not_ready("The worker account or resources changed before its conversation could reopen."));
            }
            let builtin_web = resolved.as_ref().is_some_and(|r| r.builtin_web);
            let resource_files = match resolved {
                Some(resources) => thingmaker_supervisor::integrations::member_resources::apply_to_launch(resources, &mut launch)?,
                None => None,
            };
            let program = super::session::program_of(&target, &launch);
            let mut config = SessionActorConfig::new(target, launch);
            config.account_id = Some(account.clone());
            config.builtin_web = builtin_web;
            config.resource_files = resource_files;
            config.environment = super::session::launch_environment(&state).with_program_dir(&program);
            config.before_prompt = Some(super::review::turn_snapshots(&state, &record.id, &record.canonical_root));
            // The worker's own reduced team server: the project memory and the
            // run's board (ADR-010). A provider that cannot load one as a
            // worker goes without: Gemini, which stays on its stream-json
            // bridge here.
            if let (Some(token), Some(socket), Some(program)) = (spec.mcp_token.as_deref(), state.delegation_socket.get(), relay_program())
                && spec.slot.provider.worker_team_server()
            {
                config.mcp_servers = vec![server_entry(&program, &[RELAY_ARG.to_string()], socket.path(), token)];
                if spec.slot.provider == Provider::Claude {
                    config.environment = config.environment.clone().with_set("MCP_TOOL_TIMEOUT", CLAUDE_MCP_TOOL_TIMEOUT_MS);
                }
            }
            super::team_mcp::prepare(&state, &mut config).await?;
            #[cfg(windows)]
            super::local_mcp::prepare(&state, &mut config).await?;
            let web = super::web::prepare(&state, &mut config).await;
            let actor = SessionActor::open(config).await?;
            let snapshot = actor.snapshot().await?;
            let recorded = snapshot.agent_session_id.clone().unwrap_or_else(|| reserved.as_str().to_string());
            let row = state
                .with_storage(|s| s.session_upsert_with_resources(&record.id, &recorded, SessionOrigin::Desktop, spec.slot.provider, spec.slot.resources.as_ref()))
                .map_err(storage_error)?;
            if spec.resume.is_some() {
                state.with_storage(|s| s.session_set_archive(&row.id, thingmaker_supervisor::storage::workspaces::ArchiveState::Active)).map_err(storage_error)?;
            }
            // Opened on its leader's pinned account: the worker's own
            // conversation is reopened on it too.
            if state.pinned_account(&spec.orchestrator).is_some_and(|(provider, _)| provider == spec.slot.provider) {
                state.with_storage(|s| s.setting_set(thingmaker_supervisor::storage::workspaces::SESSION_ACCOUNT, &row.id, &account)).map_err(storage_error)?;
            }
            if spec.slot.provider == Provider::Local && let Some(model) = &spec.slot.model {
                let setup = super::session::LocalSessionSetup { model: model.clone(), resources: spec.slot.resources.clone() };
                state.with_storage(|s| s.setting_set("localSessionSetup", &row.id, &setup)).map_err(storage_error)?;
            }
            let parent_agent_id = state.agent_session_id(&spec.orchestrator);
            let parent = state.with_storage(|s| s.session_by_agent_id(&record.id, &parent_agent_id)).ok().flatten();
            let _ = state.with_storage(|s| {
                s.session_set_parent(&row.id, parent.as_ref().map(|parent| parent.id.as_str()))?;
                if spec.resume.is_some() { Ok(()) } else { s.session_set_title_overlay(&row.id, Some(&spec.title)) }
            });
            super::web::attach(&state, &actor, web);
            state.insert_actor(actor.handle().id.clone(), recorded, actor.clone(), spec.root.clone());
            state.set_actor_model(&actor.handle().id, spec.slot.model.as_deref());
            Ok((actor, Some(revision)))
        })
    }

    /// The guard a resume from the sidebar gets (`session_open`), for a
    /// worker reopened to take a chat message.
    fn attached(&self, agent_session_id: &str) -> bool {
        self.app.try_state::<AppState>().is_some_and(|state| state.conversation_attached(agent_session_id))
    }

    fn released(&self, worker_handle: &str) {
        if let Some(state) = self.app.try_state::<AppState>() {
            // A closed worker's session is archived: its job is over, and the
            // sidebar keeps it under Archived rather than among live work.
            let agent_session_id = state.agent_session_id(worker_handle);
            let root = state.actor_entries().into_iter().find(|(id, _)| id == worker_handle).map(|(_, root)| root);
            if let Some(root) = root
                && let Ok(Some(workspace)) = state.with_storage(|s| s.workspace_by_root(&root.to_string_lossy()))
                && let Ok(Some(row)) = state.with_storage(|s| s.session_by_agent_id(&workspace.id, &agent_session_id))
            {
                let _ = state.with_storage(|s| s.session_set_archive(&row.id, thingmaker_supervisor::storage::workspaces::ArchiveState::Archived));
            }
            state.remove_actor(worker_handle);
        }
    }
}

fn delegation(state: &AppState) -> CommandResult<&Delegation> {
    state.delegation.get().ok_or_else(|| DesktopError::not_ready("delegation is not available: the team socket did not start"))
}

/// The desktop row of a live attachment, for persisting its team.
fn row_id(state: &AppState, handle: &SessionHandle) -> Option<String> {
    let agent_id = state.agent_session_id(&handle.id);
    let (_, root) = state.actor_entries().into_iter().find(|(id, _)| *id == handle.id)?;
    let workspace = state.with_storage(|s| s.workspace_by_root(&root.to_string_lossy())).ok().flatten()?;
    state.with_storage(|s| s.session_by_agent_id(&workspace.id, &agent_id)).ok().flatten().map(|row| row.id)
}

/// A live session's team.
#[tauri::command]
pub fn delegation_combo_get(handle: SessionHandle, state: State<'_, AppState>) -> CommandResult<Option<Combo>> {
    Ok(delegation(&state)?.combo(&handle.id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComboSetRequest {
    pub handle: SessionHandle,
    pub combo: Combo,
}

/// Changes a live session's team; the next `delegate` uses it. Kept for the
/// session's next attachment too.
#[tauri::command]
pub fn delegation_combo_set(request: ComboSetRequest, state: State<'_, AppState>) -> CommandResult<Combo> {
    if request.combo.workers.len() > 16 {
        return Err(DesktopError::io("a team has at most 16 workers"));
    }
    let combo = delegation(&state)?.set_combo(&request.handle.id, request.combo)?;
    if let Some(row) = row_id(&state, &request.handle) {
        state.with_storage(|s| s.setting_set(COMBO_KEY, &session_scope(&row), &combo)).map_err(storage_error)?;
    }
    Ok(combo)
}

/// The team a new session starts with.
#[tauri::command]
pub fn delegation_default_combo_get(state: State<'_, AppState>) -> CommandResult<Combo> {
    Ok(state.with_storage(|s| s.setting_get::<Combo>(COMBO_KEY, GLOBAL_SCOPE)).map_err(storage_error)?.unwrap_or_default())
}

#[tauri::command]
pub fn delegation_default_combo_set(combo: Combo, state: State<'_, AppState>) -> CommandResult<Combo> {
    let combo = combo.normalized();
    state.with_storage(|s| s.setting_set(COMBO_KEY, GLOBAL_SCOPE, &combo)).map_err(storage_error)?;
    Ok(combo)
}

/// A session's agent chat as the renderer reads it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamChat {
    #[serde(flatten)]
    view: ChatView,
    /// The agent messages written into this session's own conversation.
    received: Vec<Receipt>,
}

/// A live session's agent chat: the messages of the team it leads and
/// whether its agents may still send, and the receipts of the messages its
/// own conversation was sent (a worker's has those and no team).
#[tauri::command]
pub fn delegation_chat(handle: SessionHandle, state: State<'_, AppState>) -> CommandResult<TeamChat> {
    // The receipts are read whether or not the team service is up: a
    // conversation reopened without it still holds the messages it was sent.
    let view = state.delegation.get().map(|delegation| delegation.chat_view(&handle.id)).unwrap_or_default();
    Ok(TeamChat { view, received: receipts(&state, &handle.id) })
}

/// A live session's jobs, oldest first.
#[tauri::command]
pub fn delegation_jobs(handle: SessionHandle, state: State<'_, AppState>) -> CommandResult<Vec<JobView>> {
    Ok(delegation(&state)?.jobs(&handle.id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCancelRequest {
    pub handle: SessionHandle,
    pub job_id: String,
}

#[tauri::command]
pub async fn delegation_job_cancel(request: JobCancelRequest, state: State<'_, AppState>) -> CommandResult<JobView> {
    delegation(&state)?.cancel(&request.handle.id, &request.job_id).await.map_err(DesktopError::not_ready)
}

/// Whether a provider is spent as far as the router knows, for the picker.
#[tauri::command]
pub fn delegation_quota(provider: Provider, state: State<'_, AppState>) -> CommandResult<Option<thingmaker_supervisor::agents::events::QuotaSnapshot>> {
    Ok(delegation(&state)?.quota(provider))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRetryRequest {
    pub handle: SessionHandle,
    pub job_id: String,
}

/// Ends a waiting job's wait now.
#[tauri::command]
pub fn delegation_job_retry(request: JobRetryRequest, state: State<'_, AppState>) -> CommandResult<JobView> {
    delegation(&state)?.retry_now(&request.handle.id, &request.job_id).map_err(DesktopError::not_ready)
}

/// What a job does after a temporary limit.
#[tauri::command]
pub fn delegation_retry_policy_get(state: State<'_, AppState>) -> CommandResult<RetryPolicy> {
    Ok(delegation(&state)?.retry_policy())
}

#[tauri::command]
pub fn delegation_retry_policy_set(policy: RetryPolicy, state: State<'_, AppState>) -> CommandResult<RetryPolicy> {
    let policy = RetryPolicy {
        first_delay_secs: policy.first_delay_secs.clamp(30, 6 * 3600),
        step_secs: policy.step_secs.min(6 * 3600),
        max_delay_secs: policy.max_delay_secs.clamp(30, 12 * 3600),
        max_attempts: policy.max_attempts.min(50),
        max_wait_secs: policy.max_wait_secs.clamp(60, 24 * 3600),
        ..policy
    };
    state.with_storage(|s| s.setting_set(RETRY_KEY, GLOBAL_SCOPE, &policy)).map_err(storage_error)?;
    delegation(&state)?.set_retry_policy(policy);
    Ok(policy)
}

#[cfg(test)]
mod chat_receipt_tests {
    use super::*;

    fn receipt(token: &str) -> Receipt {
        Receipt { token: token.into(), from: "job-2".into(), from_name: "Builder".into(), from_provider: Provider::Codex, sent_at: 1 }
    }

    #[test]
    fn receipts_are_kept_per_conversation_once_each_and_outlive_the_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        keep_receipt(&state, "lead-conversation", receipt("a"));
        keep_receipt(&state, "lead-conversation", receipt("a"));
        keep_receipt(&state, "lead-conversation", receipt("b"));
        keep_receipt(&state, "worker-conversation", receipt("c"));
        assert_eq!(receipts(&state, "lead-conversation"), [receipt("a"), receipt("b")]);
        assert_eq!(receipts(&state, "worker-conversation"), [receipt("c")]);
        assert!(receipts(&state, "someone-else").is_empty(), "a token means nothing in another conversation");
        // The application restarts: the conversation is reopened from the
        // provider's history, and its receipts are still there.
        drop(state);
        let state = AppState::initialize(dir.path(), None).unwrap();
        assert_eq!(receipts(&state, "lead-conversation"), [receipt("a"), receipt("b")]);
        let reply = serde_json::to_value(TeamChat { view: ChatView::default(), received: receipts(&state, "lead-conversation") }).unwrap();
        assert_eq!(reply["received"][0], serde_json::json!({ "token": "a", "from": "job-2", "fromName": "Builder", "fromProvider": "codex", "sentAt": 1 }));
        assert!(reply["messages"].as_array().unwrap().is_empty(), "the team's own chat sits beside them: {reply}");
    }

    #[test]
    fn only_the_newest_receipts_of_a_conversation_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let scope = receipts_scope("long");
        let full: Vec<Receipt> = (0..CHAT_RECEIPTS_KEPT).map(|n| receipt(&format!("t{n}"))).collect();
        state.with_storage(|s| s.setting_set(CHAT_RECEIPTS_KEY, &scope, &full)).unwrap();
        keep_receipt(&state, "long", receipt("newest"));
        let kept = receipts(&state, "long");
        assert_eq!(kept.len(), CHAT_RECEIPTS_KEPT);
        assert_eq!(kept[0].token, "t1");
        assert_eq!(kept.last().unwrap().token, "newest");
    }
}

#[cfg(test)]
mod leader_tests {
    use super::*;
    use thingmaker_supervisor::{
        acp::LaunchTarget,
        agents::{accounts, gemini::GeminiLaunchOptions},
        delegation::WorkerSlot,
    };

    fn helper() -> Combo {
        Combo { workers: vec![WorkerSlot { name: "helper".into(), provider: Provider::Codex, model: None, effort: None, capabilities: Vec::new(), note: None, prompt: None, resources: None }], ..Default::default() }
    }

    #[test]
    fn gemini_runs_natively_only_when_it_starts_with_a_team_and_a_conversation_keeps_its_transport() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let workspace = state.with_storage(|s| s.workspace_upsert("/project", "/project", "project")).unwrap();
        let row = |id: &str| state.with_storage(|s| s.session_upsert(&workspace.id, id, SessionOrigin::Desktop, Provider::Gemini)).unwrap().id;
        // A new session: only with a team to lead.
        assert!(gemini_native(&state, None, &helper()));
        assert!(!gemini_native(&state, None, &Combo::default()));
        // A conversation from before, or one started without a team, stays on
        // the stream-json bridge even once it has been given a team.
        let old = row("stream-json");
        assert!(!gemini_native(&state, Some(&old), &helper()));
        // One that began natively is reopened natively, with or without one.
        let native = row("native");
        state.with_storage(|s| s.setting_set(GEMINI_NATIVE_KEY, &native, &true)).unwrap();
        assert!(gemini_native(&state, Some(&native), &Combo::default()));
    }

    #[test]
    fn a_plain_gemini_session_is_left_exactly_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let options = GeminiLaunchOptions::new(dir.path(), "agy");
        let before = options.arguments();
        let mut launch = AgentLaunch::Gemini(options);
        let mut config = SessionActorConfig::new(LaunchTarget::executable("agy"), launch.clone());
        assert!(!prepare_orchestrator(&state, "plain", dir.path().into(), &helper(), &mut launch, &mut config, false));
        let AgentLaunch::Gemini(options) = &launch else { panic!("wrong provider") };
        assert!(!options.native_acp && options.initial_prompt_prefix.is_none());
        assert_eq!(options.arguments(), before, "still the stream-json command line");
        assert!(config.mcp_servers.is_empty() && !config.leads_team);
        // The transport is the conversation's even when the team service is
        // down: a native one is not reopened on the bridge.
        assert!(!prepare_orchestrator(&state, "native", dir.path().into(), &helper(), &mut launch, &mut config, true));
        let AgentLaunch::Gemini(options) = &launch else { panic!("wrong provider") };
        assert!(options.native_acp);
        assert_eq!(options.arguments()[0], "acp");
    }

    #[test]
    fn a_worker_runs_on_its_leaders_pinned_account_only_on_the_leaders_provider() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        let list = [("work", Provider::Claude), ("side", Provider::Codex)].map(|(id, provider)| accounts::Account { id: id.into(), provider, label: id.into() });
        state.with_storage(|s| s.setting_set(accounts::ACCOUNTS_SETTING, GLOBAL_SCOPE, &list)).unwrap();
        state.with_storage(|s| s.setting_set(accounts::ACTIVE_ACCOUNT_SETTING, "codex", &"side")).unwrap();
        // A leader that follows the active account: so do its workers.
        assert_eq!(worker_account(&state, "lead", Provider::Claude), "default");
        // Pinned to another Claude account, after the first one ran out.
        state.set_pinned_account("lead", Provider::Claude, "work");
        assert_eq!(worker_account(&state, "lead", Provider::Claude), "work");
        assert_eq!(worker_account(&state, "lead", Provider::Codex), "side", "another provider's worker keeps that provider's active account");
        assert_eq!(worker_account(&state, "someone-else", Provider::Claude), "default");
        // The revision a worker is reused by names the account it ran on.
        assert_ne!(revision_of("work", dir.path(), None), revision_of("default", dir.path(), None));
    }
}
