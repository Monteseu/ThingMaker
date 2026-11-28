//! Application state owned by the Tauri host.
//!
//! Session actors live here, not in React components (F01): closing a pane
//! only drops a subscription. Storage access is serialized through a mutex
//! with short critical sections (no awaits while held).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use thingmaker_supervisor::{
    DesktopError,
    agents::{Provider, claude, gemini},
    storage::Storage,
    supervisor::SessionActor,
    terminal::TerminalManager,
};

/// Settings key (scope `providers`) for an explicit program location.
pub const PROVIDER_LOCATION_KEY: &str = "location";

/// Where a provider's program is, as chosen in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLocation {
    /// The executable: Node for Claude's adapter, `codex` for Codex.
    pub program: PathBuf,
    /// Claude only: the adapter's `dist/index.js`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<PathBuf>,
}

/// A provider's program, resolved: what to run, and the arguments that come
/// before anything the session adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedProvider {
    pub provider: Provider,
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
    /// How it was found: `settings`, `environment` or `discovered`.
    pub source: &'static str,
}

pub struct AppState {
    pub web_sessions: Mutex<HashMap<String, Result<crate::commands::web::Connection, String>>>,
    pub storage: Arc<Mutex<Storage>>,
    pub actors: Mutex<HashMap<String, SessionActor>>,
    /// Canonical root per live session id, for worktree cleanup safety.
    pub actor_roots: Mutex<HashMap<String, PathBuf>>,
    /// The id the *agent* knows a live attachment by, per attachment handle.
    ///
    /// Agents choose their own session ids and only say so in the
    /// `session/new` reply, so the handle the desktop addresses a new
    /// attachment by is not the id its session row, its transcript or its
    /// resume are under. Anything that reaches storage has to go through this
    /// (docs/plans/odyssey-second-orchestrator.md §2.1).
    pub agent_session_ids: Mutex<HashMap<String, String>>,
    /// Cancellation handles for in-progress provider sign-ins by run id.
    pub auth_runs: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>,
    pub home: Option<PathBuf>,
    pub terminals: TerminalManager,
    pub terminal_workspaces: Mutex<HashMap<String, String>>,
    /// Serializes login terminal creation with renderer reload cleanup.
    pub auth_terminal_generation: Mutex<u64>,
    pub auth_terminal_profiles: Mutex<HashMap<String, PathBuf>>,
    pub account_preparation: Mutex<()>,
    pub data_dir: PathBuf,
    /// Tauri resource directory in packaged builds.
    pub resource_dir: Option<PathBuf>,
    /// Teams and jobs, and the socket agents' ThingMaker MCP children reach
    /// them on. Set once in `setup`.
    pub delegation: std::sync::OnceLock<thingmaker_supervisor::delegation::Delegation>,
    pub delegation_socket: std::sync::OnceLock<thingmaker_supervisor::delegation::socket::DelegationSocket>,
    /// Big Thing's engine (ADR-010). Set once in `setup`, after delegation.
    pub engine: std::sync::OnceLock<thingmaker_supervisor::bigthing::Engine>,
    /// Ultra-Review. Set once in `setup`, after delegation.
    pub ultra_review: std::sync::OnceLock<thingmaker_supervisor::ultrareview::UltraReviewService>,
    /// The model each live attachment was opened on, when one was asked for.
    pub actor_models: Mutex<HashMap<String, String>>,
    /// The account each live attachment was pinned to, with its provider,
    /// when the person chose one for the session rather than the provider's
    /// active account.
    pub pinned_accounts: Mutex<HashMap<String, (Provider, String)>>,
    /// Shared by sessions in the same tree; hooks own the cache's lifetime.
    pub review_caches: Mutex<HashMap<PathBuf, std::sync::Weak<Mutex<thingmaker_supervisor::review::BaselineCache>>>>,
    /// The update the last check found, kept so installing is the one the person saw.
    pub pending_update: Mutex<Option<tauri_plugin_updater::Update>>,
}

impl AppState {
    pub fn initialize(data_dir: &Path, resource_dir: Option<&Path>) -> Result<Self, String> {
        let storage = Storage::open(&data_dir.join("thingmaker.db")).map_err(|e| e.to_string())?;
        tracing::info!(?data_dir, "desktop state initialized");
        let storage = Arc::new(Mutex::new(storage));
        Ok(Self {
            web_sessions: Mutex::new(HashMap::new()),
            review_caches: Mutex::new(HashMap::new()),
            pending_update: Mutex::new(None),
            storage,
            actors: Mutex::new(HashMap::new()),
            agent_session_ids: Mutex::new(HashMap::new()),
            actor_roots: Mutex::new(HashMap::new()),
            auth_runs: Arc::new(Mutex::new(HashMap::new())),
            home: std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from),
            terminals: TerminalManager::new(),
            terminal_workspaces: Mutex::new(HashMap::new()),
            auth_terminal_generation: Mutex::new(0),
            auth_terminal_profiles: Mutex::new(HashMap::new()),
            account_preparation: Mutex::new(()),
            data_dir: data_dir.to_path_buf(),
            resource_dir: resource_dir.map(Path::to_path_buf),
            delegation: std::sync::OnceLock::new(),
            delegation_socket: std::sync::OnceLock::new(),
            engine: std::sync::OnceLock::new(),
            ultra_review: std::sync::OnceLock::new(),
            actor_models: Mutex::new(HashMap::new()),
            pinned_accounts: Mutex::new(HashMap::new()),
        })
    }

    pub fn with_storage<T>(&self, f: impl FnOnce(&Storage) -> T) -> T {
        let guard = self.storage.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard)
    }

    pub fn actor(&self, id: &str) -> Option<SessionActor> {
        self.actors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    /// Registers a live attachment. `agent_session_id` is the id the agent
    /// itself uses.
    pub fn insert_actor(&self, id: String, agent_session_id: String, actor: SessionActor, root: PathBuf) {
        crate::titles::observe(Arc::clone(&self.storage), self.home.clone(), actor.clone(), agent_session_id.clone(), root.clone());
        self.actor_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), root);
        self.agent_session_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), agent_session_id);
        self.actors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, actor);
    }

    /// The id the agent knows this attachment by, for anything that has to
    /// find its session row, its transcript or its resume. Falls back to the
    /// handle, which is right for a resumed attachment (its handle is the
    /// agent's id).
    pub fn agent_session_id(&self, id: &str) -> String {
        self.agent_session_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    /// Remembers the model an attachment was opened on.
    pub fn set_actor_model(&self, id: &str, model: Option<&str>) {
        let mut models = self.actor_models.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match model {
            Some(model) => {
                models.insert(id.to_string(), model.to_string());
            }
            None => {
                models.remove(id);
            }
        }
    }

    pub fn actor_model(&self, id: &str) -> Option<String> {
        self.actor_models.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(id).cloned()
    }

    /// Remembers the account an attachment was pinned to.
    pub fn set_pinned_account(&self, id: &str, provider: Provider, account: &str) {
        self.pinned_accounts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(id.to_string(), (provider, account.to_string()));
    }

    /// The account a live attachment is pinned to, and the provider it is
    /// an account of; `None` for one that follows the active account.
    pub fn pinned_account(&self, id: &str) -> Option<(Provider, String)> {
        self.pinned_accounts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(id).cloned()
    }

    pub fn remove_actor(&self, id: &str) -> Option<SessionActor> {
        self.web_sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        self.actor_models.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
        self.pinned_accounts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
        self.actor_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
        self.agent_session_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
        self.actors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id)
    }

    /// Live sessions with their canonical roots.
    pub fn actor_entries(&self) -> Vec<(String, PathBuf)> {
        self.actor_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, root)| (id.clone(), root.clone()))
            .collect()
    }

    /// Live session ids whose root is `root` (or inside it).
    pub fn actors_under(&self, root: &Path) -> Vec<String> {
        self.actor_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(_, path)| path.starts_with(root))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Storage handle for blocking background tasks.
    pub fn inner_arc(&self) -> StorageHandle {
        StorageHandle(Arc::clone(&self.storage))
    }

    pub fn auth_runs_arc(&self) -> Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>> {
        Arc::clone(&self.auth_runs)
    }

    /// Whether a live attachment is already on this conversation, by its
    /// handle or by the id its agent keeps it under. A second process on it
    /// would write the same transcript.
    pub fn conversation_attached(&self, agent_session_id: &str) -> bool {
        self.agent_session_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|(handle, agent)| handle == agent_session_id || agent == agent_session_id)
    }

    pub fn actor_ids(&self) -> Vec<String> {
        self.actors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

impl AppState {
    /// The explicit location chosen in Settings for a provider, if any.
    pub fn provider_location(&self, provider: Provider) -> Option<ProviderLocation> {
        self.with_storage(|s| s.setting_get::<ProviderLocation>(PROVIDER_LOCATION_KEY, &format!("provider:{}", provider.as_str())))
            .ok()
            .flatten()
    }

    /// Finds a provider's program. Resolved each time it is needed rather
    /// than once at startup, so installing or updating a CLI takes effect on
    /// the next session without restarting the app.
    pub fn resolve_provider(&self, provider: Provider) -> Result<ResolvedProvider, DesktopError> {
        let configured = self.provider_location(provider);
        let home = self.home.clone().unwrap_or_default();
        match provider {
            Provider::Claude => {
                let chosen = configured.as_ref().and_then(|location| Some((location.program.clone(), location.script.clone()?)));
                let source = if chosen.is_some() {
                    "settings"
                } else if std::env::var_os(claude::launch::ADAPTER_ENV).is_some() {
                    "environment"
                } else {
                    "discovered"
                };
                let (node, script) = claude::launch::locate_adapter(&home, chosen, std::env::var(claude::launch::ADAPTER_ENV).ok())?;
                Ok(ResolvedProvider {
                    provider,
                    program: node,
                    prefix_args: vec![script.to_string_lossy().into_owned()],
                    source,
                })
            }
            Provider::Codex => {
                if let Some(location) = configured {
                    return Ok(ResolvedProvider { provider, program: location.program, prefix_args: Vec::new(), source: "settings" });
                }
                if let Some(path) = std::env::var_os("THINGMAKER_CODEX").map(PathBuf::from).filter(|path| path.is_file()) {
                    return Ok(ResolvedProvider { provider, program: path, prefix_args: Vec::new(), source: "environment" });
                }
                let program = find_codex(&home).ok_or_else(|| {
                    DesktopError::not_ready(
                        "Codex is not installed. Install the Codex CLI (`npm install -g @openai/codex`) or the ChatGPT app, or choose the `codex` binary in Settings.",
                    )
                })?;
                Ok(ResolvedProvider { provider, program, prefix_args: Vec::new(), source: "discovered" })
            }
            Provider::Copilot => {
                let override_path = std::env::var_os(thingmaker_supervisor::agents::copilot::launch::PROGRAM_ENV).map(PathBuf::from);
                let source = if configured.is_some() { "settings" } else if override_path.is_some() { "environment" } else { "discovered" };
                let program = thingmaker_supervisor::agents::copilot::launch::locate(&home, configured.as_ref().map(|location| location.program.as_path()), override_path)
                    .ok_or_else(|| DesktopError::not_ready("GitHub Copilot CLI is not installed. Install `@github/copilot` or set THINGMAKER_COPILOT, then run `copilot login`."))?;
                Ok(ResolvedProvider { provider, program, prefix_args: Vec::new(), source })
            }
            Provider::Gemini => {
                if let Some(location) = configured {
                    return Ok(ResolvedProvider { provider, program: location.program, prefix_args: Vec::new(), source: "settings" });
                }
                if let Some(path) = std::env::var_os(gemini::launch::PROGRAM_ENV).map(PathBuf::from).filter(|path| path.is_file()) {
                    return Ok(ResolvedProvider { provider, program: path, prefix_args: Vec::new(), source: "environment" });
                }
                let program = gemini::launch::find_program(&home).ok_or_else(|| {
                    DesktopError::not_ready(
                        "Antigravity CLI (agy) is not installed. Install it with `curl -fsSL https://antigravity.google/cli/install.sh | bash`, or choose the `agy` binary in Settings.",
                    )
                })?;
                Ok(ResolvedProvider { provider, program, prefix_args: Vec::new(), source: "discovered" })
            }
            // Local Models: Ollama's own program. A local agent also needs
            // the Claude adapter, which its launch resolves for itself.
            Provider::Local => {
                if let Some(location) = configured {
                    return Ok(ResolvedProvider { provider, program: location.program, prefix_args: Vec::new(), source: "settings" });
                }
                let program = thingmaker_supervisor::agents::local::find_ollama(&home)
                    .ok_or_else(|| DesktopError::not_ready("Ollama is not installed. Get it from ollama.com/download to run models on this computer."))?;
                Ok(ResolvedProvider { provider, program, prefix_args: Vec::new(), source: "discovered" })
            }
        }
    }
}

impl AppState {
    /// The accounts added beside the person's own, for every provider.
    pub fn accounts(&self) -> Vec<thingmaker_supervisor::agents::accounts::Account> {
        use thingmaker_supervisor::{agents::accounts, storage::settings::GLOBAL_SCOPE};
        accounts::read_accounts(self.with_storage(|s| s.setting_get::<serde_json::Value>(accounts::ACCOUNTS_SETTING, GLOBAL_SCOPE)).ok().flatten())
    }

    /// A provider's active account: an added one's id, or the default.
    pub fn active_account(&self, provider: Provider) -> String {
        use thingmaker_supervisor::agents::accounts;
        let id = self.with_storage(|s| s.setting_get::<String>(accounts::ACTIVE_ACCOUNT_SETTING, provider.as_str())).ok().flatten();
        id.filter(|id| self.accounts().iter().any(|account| account.provider == provider && &account.id == id)).unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.to_string())
    }

    /// Resolve once before launching, so later global account changes cannot
    /// redirect a session. Explicit invalid accounts never select the default.
    pub fn session_account_id(&self, provider: Provider, requested: Option<&str>) -> Result<String, DesktopError> {
        use thingmaker_supervisor::agents::accounts;
        let id = requested.map(str::to_string).unwrap_or_else(|| self.active_account(provider));
        if id == accounts::DEFAULT_ACCOUNT { return Ok(id); }
        if !accounts::valid_id(&id) { return Err(DesktopError::unsupported("invalid account id")); }
        if !accounts::supports_accounts(provider) || !self.accounts().iter().any(|account| account.provider == provider && account.id == id) {
            return Err(DesktopError::not_ready("that account does not belong to this provider or is gone"));
        }
        Ok(id)
    }

    /// The settings folder of account `id` (the active one when `None`),
    /// shared with the person's main folder again before it is used. `None`
    /// for the default account, which is the person's own folder.
    pub fn try_account_folder(&self, provider: Provider, id: Option<&str>) -> Result<Option<PathBuf>, DesktopError> {
        use thingmaker_supervisor::agents::accounts;
        let id = self.session_account_id(provider, id)?;
        if id == accounts::DEFAULT_ACCOUNT {
            return Ok(None);
        }
        let home = self.home.clone().ok_or_else(|| DesktopError::not_ready("home directory unavailable"))?;
        let folder = accounts::account_folder(&self.data_dir, provider, &id);
        let _preparation = self.account_preparation.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(main) = accounts::main_folder(provider, &home)
            && let Err(error) = accounts::share(provider, &main, &folder, &home)
        {
            return Err(DesktopError::io(format!("could not prepare the account: {error}")));
        }
        Ok(Some(folder))
    }
}

/// `codex` on PATH, in the usual global install prefixes, or the one the
/// ChatGPT and Codex apps ship. A desktop app launched from the Dock has no
/// shell PATH, so the prefixes are searched explicitly.
fn find_codex(home: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = std::env::split_paths(&path_var).map(|dir| dir.join(name)).collect();
    let mut nvm: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("bin").join(name))
        .collect();
    nvm.sort();
    nvm.reverse();
    candidates.extend(nvm);
    #[cfg(windows)]
    {
        // The Codex desktop installer keeps versioned native binaries here, newest first.
        let mut bundled: Vec<_> = std::fs::read_dir(home.join("AppData/Local/OpenAI/Codex/bin")).into_iter().flatten().flatten().map(|entry| entry.path().join(name)).collect();
        bundled.sort_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok());
        candidates.extend(bundled.into_iter().rev());
        for dir in std::env::split_paths(&path_var).chain([home.join("AppData/Roaming/npm")]) {
            candidates.push(dir.join("node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe"));
            candidates.push(dir.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe"));
        }
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        home.join(".local/bin").join(name),
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin").join(name),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex-cli/bin").join(name),
    ]);
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Cloneable storage access for tasks that outlive a command.
#[derive(Clone)]
pub struct StorageHandle(Arc<Mutex<Storage>>);

impl StorageHandle {
    pub fn with_storage<T>(&self, f: impl FnOnce(&Storage) -> T) -> T {
        let guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard)
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use thingmaker_supervisor::{agents::accounts, storage::settings::GLOBAL_SCOPE};

    #[test]
    fn a_broken_gemini_profile_is_an_error_instead_of_the_main_account() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::initialize(dir.path(), None).unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini/config"), "not a directory").unwrap();
        state.home = Some(home);
        let list = vec![accounts::Account { id: "acct-test".into(), provider: Provider::Gemini, label: "Test".into() }];
        state.with_storage(|s| s.setting_set(accounts::ACCOUNTS_SETTING, GLOBAL_SCOPE, &list)).unwrap();
        assert!(state.try_account_folder(Provider::Gemini, Some("acct-test")).is_err());
        assert!(state.try_account_folder(Provider::Gemini, Some("default")).unwrap().is_none());
    }

    #[test]
    fn claude_and_codex_report_unusable_account_folders() {
        for provider in [Provider::Claude, Provider::Codex] {
            let dir = tempfile::tempdir().unwrap();
            let mut state = AppState::initialize(dir.path(), None).unwrap();
            state.home = Some(dir.path().join("home"));
            let account = accounts::Account { id: "work".into(), provider, label: "Work".into() };
            state.with_storage(|s| s.setting_set(accounts::ACCOUNTS_SETTING, GLOBAL_SCOPE, &[account])).unwrap();
            let folder = accounts::account_folder(dir.path(), provider, "work");
            std::fs::create_dir_all(folder.parent().unwrap()).unwrap();
            std::fs::write(folder, "not a directory").unwrap();
            assert!(state.try_account_folder(provider, Some("work")).is_err());
        }
    }

    #[test]
    fn a_conversation_is_attached_under_its_handle_or_the_agents_own_id() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        assert!(!state.conversation_attached("agent-1"));
        // A worker a delegation opened: a reserved handle, the agent's id apart.
        state.agent_session_ids.lock().unwrap().insert("reserved-handle".into(), "agent-1".into());
        assert!(state.conversation_attached("agent-1"), "resuming it would start a second process");
        assert!(state.conversation_attached("reserved-handle"));
        assert!(!state.conversation_attached("agent-2"));
        state.remove_actor("reserved-handle");
        assert!(!state.conversation_attached("agent-1"), "closed: free to reopen");
    }

    #[test]
    fn invalid_or_removed_accounts_cannot_fall_back_to_the_main_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::initialize(dir.path(), None).unwrap();
        for id in ["../default", "acct-removed"] {
            assert!(state.try_account_folder(Provider::Gemini, Some(id)).is_err());
        }
    }
}
