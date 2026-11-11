//! Local Models: Ollama's state for the Providers card, starting Ollama,
//! and downloading and removing models.
//!
//! Every call goes to Ollama on `127.0.0.1`. Nothing here reaches the
//! internet except a download Ollama itself makes, which the person asked for.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{ipc::Channel, State};
use thingmaker_supervisor::{
    agents::local::{
        self,
        ollama::{LoadedModel, LocalModel, Ollama, PullProgress},
        recommended::{self, Fit, Recommended},
    },
    error::DesktopError,
};

use super::CommandResult;
use crate::state::AppState;

/// How private mode is enforced for local agents on this system.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateMode {
    /// macOS: `sandbox-exec`.
    Sandbox,
    /// Windows: an AppContainer.
    AppContainer,
    /// Neither: local agents do not start; chats still work.
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendedModel {
    #[serde(flatten)]
    pub model: Recommended,
    pub installed: bool,
    pub fit: Option<Fit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledModel {
    #[serde(flatten)]
    pub model: LocalModel,
    pub fit: Option<Fit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    pub installed: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub models: Vec<InstalledModel>,
    pub loaded: Vec<LoadedModel>,
    pub recommended: Vec<RecommendedModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    pub private_mode: PrivateMode,
    /// Windows: whether the AppContainer may reach Ollama yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loopback_allowed: Option<bool>,
    /// Whether Claude Code's adapter is there for local agents.
    pub agents_available: bool,
}

fn private_mode() -> PrivateMode {
    if cfg!(target_os = "macos") && std::path::Path::new("/usr/bin/sandbox-exec").is_file() {
        PrivateMode::Sandbox
    } else if cfg!(windows) {
        PrivateMode::AppContainer
    } else {
        PrivateMode::Unavailable
    }
}

/// Ollama's state: installed, running, the models and what fits.
#[tauri::command]
pub async fn local_status(state: State<'_, AppState>) -> CommandResult<LocalStatus> {
    let installed = state.resolve_provider(thingmaker_supervisor::agents::Provider::Local).is_ok();
    let agents_available = state.resolve_provider(thingmaker_supervisor::agents::Provider::Claude).is_ok();
    tauri::async_runtime::spawn_blocking(move || {
        let ollama = Ollama::local();
        let memory = local::memory_bytes();
        let fit = |size: u64| memory.map(|memory| recommended::fit(size, memory));
        let (running, version, problem) = match ollama.version() {
            Ok(version) => (true, Some(version), None),
            Err(problem) => (false, None, Some(problem)),
        };
        let models = if running { ollama.models().unwrap_or_default() } else { Vec::new() };
        let loaded = if running { ollama.loaded().unwrap_or_default() } else { Vec::new() };
        let recommended = recommended::RECOMMENDED
            .iter()
            .map(|model| RecommendedModel { model: model.clone(), installed: models.iter().any(|installed| installed.name == model.name), fit: fit(model.size_bytes) })
            .collect();
        LocalStatus {
            installed,
            running,
            version,
            problem,
            models: models.into_iter().map(|model| InstalledModel { fit: fit(model.size_bytes), model }).collect(),
            loaded,
            recommended,
            memory_bytes: memory,
            private_mode: private_mode(),
            loopback_allowed: cfg!(windows).then(|| thingmaker_appcontainer::ensure_profile().map(|sid| thingmaker_appcontainer::loopback_allowed(&sid)).unwrap_or(false)),
            agents_available,
        }
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))
}

/// Starts Ollama: its app when installed, else `ollama serve` in the
/// background. Returns once it answers, or says it did not.
#[tauri::command]
pub async fn local_start(state: State<'_, AppState>) -> CommandResult<String> {
    let program = state.resolve_provider(thingmaker_supervisor::agents::Provider::Local)?.program;
    tauri::async_runtime::spawn_blocking(move || {
        let ollama = Ollama::local();
        if let Ok(version) = ollama.version() {
            return Ok(version);
        }
        let app = std::path::Path::new("/Applications/Ollama.app");
        let started = if cfg!(target_os = "macos") && app.is_dir() {
            std::process::Command::new("/usr/bin/open").args(["-g", "-a", "Ollama"]).status().map(|status| status.success()).unwrap_or(false)
        } else {
            thingmaker_supervisor::platform::command(&program)
                .arg("serve")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok()
        };
        if !started {
            return Err(DesktopError::io("Ollama could not be started"));
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(version) = ollama.version() {
                return Ok(version);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        Err(DesktopError::not_ready("Ollama started but is not answering yet; try again in a moment"))
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?
}

fn pulls() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static PULLS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    PULLS.get_or_init(Default::default)
}

/// What a download says while it runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PullEvent {
    Progress(PullProgress),
    Done,
    Cancelled,
    Failed { message: String },
}

/// Downloads a model, streaming progress. One download per model at a time.
#[tauri::command]
pub fn local_pull(name: String, on_event: Channel<PullEvent>) -> CommandResult<()> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '\0') {
        return Err(DesktopError::unsupported("that is not a model name"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = pulls().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if running.contains_key(&name) {
            return Err(DesktopError::conflict(format!("{name} is already downloading")));
        }
        running.insert(name.clone(), Arc::clone(&cancel));
    }
    std::thread::spawn(move || {
        let outcome = Ollama::local().pull(&name, &cancel, |progress| {
            let _ = on_event.send(PullEvent::Progress(progress));
        });
        let _ = on_event.send(match outcome {
            Ok(true) => PullEvent::Done,
            Ok(false) => PullEvent::Cancelled,
            Err(message) => PullEvent::Failed { message },
        });
        pulls().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&name);
    });
    Ok(())
}

#[tauri::command]
pub fn local_pull_cancel(name: String) -> CommandResult<bool> {
    let cancel = pulls().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(name.trim()).cloned();
    Ok(cancel.map(|cancel| cancel.store(true, Ordering::Relaxed)).is_some())
}

/// Removes a model, and the copies ThingMaker made of it for agents.
#[tauri::command]
pub async fn local_delete(name: String) -> CommandResult<()> {
    tauri::async_runtime::spawn_blocking(move || Ollama::local().delete(name.trim()).map_err(DesktopError::io))
        .await
        .map_err(|error| DesktopError::io(error.to_string()))?
}

/// Windows: asks, with Windows' own administrator prompt, to let local agents
/// reach Ollama on this PC. True once allowed.
#[tauri::command]
pub async fn local_allow_loopback() -> CommandResult<bool> {
    tauri::async_runtime::spawn_blocking(|| {
        let sid = thingmaker_appcontainer::ensure_profile().map_err(DesktopError::unsupported)?;
        thingmaker_appcontainer::request_loopback(&sid).map_err(DesktopError::io)
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?
}
