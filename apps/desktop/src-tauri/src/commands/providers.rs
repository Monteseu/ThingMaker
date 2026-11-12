//! Providers: where each one's program is, whether it is signed in, how to
//! sign in, and which models the account can select.
//!
//! Every answer comes from the provider's own official program. The desktop
//! runs it as a separate process, never reads or stores a token, and only
//! surfaces a sign-in URL for the user to click (SEC-06).

use std::{path::{Path, PathBuf}, time::Duration};

use serde::{Deserialize, Serialize};
use tauri::{State, ipc::Channel};
use thingmaker_supervisor::{
    DesktopError,
    agents::{
        Provider,
        auth::{ProviderAuthStatus, ProviderModel},
        auth::run_bounded,
        claude, codex, gemini,
        login::{LoginEvent, run_login_process},
        versions::{self, ProviderVersion},
    },
    security::EnvironmentProfile,
};

use super::{CommandResult, storage_error};
use crate::state::{AppState, PROVIDER_LOCATION_KEY, ProviderLocation, ResolvedProvider};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub provider: Provider,
    pub label: &'static str,
    /// Where the program is, when it was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<ResolvedProvider>,
    /// Why it could not be found or asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuthStatus>,
}

/// A provider's sign-in, for the account whose folder is `folder` (the
/// person's own when `None`).
pub(crate) fn auth_status(resolved: &ResolvedProvider, folder: Option<&Path>) -> ProviderAuthStatus {
    match resolved.provider {
        Provider::Claude => claude::auth::read_status(&resolved.program, &resolved.prefix_args, folder, Duration::from_secs(30)),
        Provider::Codex => codex::probe::read_status(&resolved.program, folder, Duration::from_secs(30)),
        Provider::Gemini => gemini::probe::read_status(&resolved.program, Duration::from_secs(30)),
        // "Signed in" for Local Models means Ollama answers: there is no
        // account. Its version stands where an account would.
        Provider::Local => match thingmaker_supervisor::agents::local::ollama::Ollama::local().version() {
            Ok(version) => ProviderAuthStatus { provider: Provider::Local, logged_in: Some(true), method: Some("ollama".into()), account: Some(format!("Ollama {version}")), plan: None, problem: None },
            Err(problem) => ProviderAuthStatus { provider: Provider::Local, logged_in: Some(false), method: Some("ollama".into()), account: None, plan: None, problem: Some(problem) },
        },
    }
}

/// Claude Code's environment for an account: its folder when it is not the
/// person's own.
fn claude_environment(folder: Option<&Path>) -> EnvironmentProfile {
    let profile = EnvironmentProfile::trusted_local();
    match folder {
        Some(dir) => profile.with_set("CLAUDE_CONFIG_DIR", dir.to_string_lossy()),
        None => profile,
    }
}

/// Every provider this build knows, with its program and sign-in state.
/// Asking can take a few seconds per provider (each answers through its own
/// CLI), so this runs off the command thread.
#[tauri::command]
pub async fn providers_status(state: State<'_, AppState>) -> CommandResult<Vec<ProviderInfo>> {
    let resolved: Vec<(Provider, Result<ResolvedProvider, DesktopError>, Option<PathBuf>)> =
        Provider::ALL.iter().map(|provider| (*provider, state.resolve_provider(*provider), state.account_folder(*provider, None))).collect();
    tauri::async_runtime::spawn_blocking(move || {
        resolved
            .into_iter()
            .map(|(provider, resolved, folder)| match resolved {
                Ok(resolved) => ProviderInfo {
                    provider,
                    label: provider.label(),
                    auth: Some(auth_status(&resolved, folder.as_deref())),
                    resolved: Some(resolved),
                    problem: None,
                },
                Err(error) => ProviderInfo { provider, label: provider.label(), resolved: None, problem: Some(error.message), auth: None },
            })
            .collect()
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))
}

/// Each provider program's installed version, the newest one where it can be
/// asked without updating (`ask_latest`; npm answers for the Claude adapter),
/// and how it updates. Asked in parallel: npm takes a second or two.
#[tauri::command]
pub async fn provider_versions(ask_latest: bool, state: State<'_, AppState>) -> CommandResult<Vec<ProviderVersion>> {
    let resolved: Vec<(Provider, Result<ResolvedProvider, DesktopError>)> =
        Provider::ALL.iter().map(|provider| (*provider, state.resolve_provider(*provider))).collect();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|scope| {
            let asks: Vec<_> = resolved
                .iter()
                .map(|(provider, resolved)| {
                    scope.spawn(move || match resolved {
                        Ok(resolved) => versions::check(*provider, &resolved.program, &resolved.prefix_args, ask_latest),
                        Err(error) => ProviderVersion::missing(*provider, error.message.clone()),
                    })
                })
                .collect();
            asks.into_iter().zip(&resolved).map(|(ask, (provider, _))| ask.join().unwrap_or_else(|_| ProviderVersion::missing(*provider, "the version check failed"))).collect()
        })
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdateResult {
    /// The tail of what the update printed.
    pub output: String,
    /// The version check after it.
    pub version: ProviderVersion,
}

/// Updates a provider's program with its own tooling (`npm install -g` for
/// the Claude adapter, `agy update`, `codex update`). Sessions already open
/// keep the program they started with; new sessions start the updated one.
#[tauri::command]
pub async fn provider_update(provider: Provider, state: State<'_, AppState>) -> CommandResult<ProviderUpdateResult> {
    let resolved = state.resolve_provider(provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        tracing::info!(provider = provider.as_str(), "updating the provider program");
        let output = versions::update(provider, &resolved.program, &resolved.prefix_args).map_err(|error| {
            tracing::warn!(provider = provider.as_str(), %error, "provider update failed");
            DesktopError::io(error)
        })?;
        let version = versions::check(provider, &resolved.program, &resolved.prefix_args, true);
        tracing::info!(provider = provider.as_str(), installed = ?version.installed, "provider program updated");
        Ok(ProviderUpdateResult { output, version })
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLocationRequest {
    pub provider: Provider,
    /// `None` clears the choice and goes back to discovery.
    pub location: Option<ProviderLocation>,
}

/// Records where a provider's program is, or clears the choice.
#[tauri::command]
pub fn provider_set_location(request: ProviderLocationRequest, state: State<'_, AppState>) -> CommandResult<()> {
    let scope = format!("provider:{}", request.provider.as_str());
    match request.location {
        Some(location) => {
            if !location.program.is_file() {
                return Err(DesktopError::not_ready(format!("{} does not exist", location.program.display())));
            }
            if let Some(script) = &location.script
                && !script.is_file()
            {
                return Err(DesktopError::not_ready(format!("{} does not exist", script.display())));
            }
            state.with_storage(|s| s.setting_set(PROVIDER_LOCATION_KEY, &scope, &location)).map_err(storage_error)
        }
        None => state.with_storage(|s| s.setting_delete(PROVIDER_LOCATION_KEY, &scope)).map_err(storage_error),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStartResponse {
    pub run_id: String,
}

/// Starts a provider's own sign-in and streams what it prints.
///
/// Claude Code signs in with `--claudeai`, which is what makes the work count
/// against a Claude *subscription* rather than metered API billing — this
/// desktop runs on subscriptions only.
#[tauri::command]
pub fn provider_login_start(provider: Provider, on_event: Channel<LoginEvent>, account: Option<String>, state: State<'_, AppState>) -> CommandResult<LoginStartResponse> {
    let resolved = state.resolve_provider(provider)?;
    // Signing in to an account is signing in inside its folder.
    let folder = state.account_folder(provider, account.as_deref());
    // Both sign in with the subscription: Claude Code's `--claudeai`, and
    // `codex login`, which is ChatGPT sign-in unless an API key is given.
    // `agy` has no headless sign-in; it shares the Antigravity app's.
    let (args, environment) = match provider {
        Provider::Claude => (claude::auth::login_args(&resolved.prefix_args, true), claude_environment(folder.as_deref())),
        Provider::Codex => (codex::probe::login_args(), codex::probe::environment(folder.as_deref()).with_program_dir(&resolved.program)),
        Provider::Gemini => return Err(DesktopError::unsupported(gemini::probe::SIGN_IN_HINT)),
        Provider::Local => return Err(DesktopError::unsupported("Local Models need no sign-in: start Ollama instead.")),
    };
    let program: PathBuf = resolved.program;
    let run_id = thingmaker_supervisor::storage::new_id();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    state.auth_runs.lock().unwrap_or_else(|p| p.into_inner()).insert(run_id.clone(), cancel_tx);
    let run_id_for_task = run_id.clone();
    let runs = state.auth_runs_arc();
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<LoginEvent>(256);
        let forward = tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                if on_event.send(event).is_err() {
                    break;
                }
            }
        });
        let outcome = run_login_process(&program, &args, &environment, Duration::from_secs(10 * 60), tx, cancel_rx).await;
        if let Err(error) = outcome {
            tracing::warn!(?error, "provider sign-in failed to run");
        }
        let _ = forward.await;
        runs.lock().unwrap_or_else(|p| p.into_inner()).remove(&run_id_for_task);
    });
    Ok(LoginStartResponse { run_id })
}

#[tauri::command]
pub fn provider_login_cancel(run_id: String, state: State<'_, AppState>) -> CommandResult<bool> {
    let sender = state.auth_runs.lock().unwrap_or_else(|p| p.into_inner()).remove(&run_id);
    Ok(match sender {
        Some(sender) => sender.send(()).is_ok(),
        None => false,
    })
}

/// The models this account can select, from the provider itself.
#[tauri::command]
pub async fn provider_models(provider: Provider, state: State<'_, AppState>) -> CommandResult<Vec<ProviderModel>> {
    let resolved = state.resolve_provider(provider)?;
    let folder = state.account_folder(provider, None);
    // The app's own data directory, not a project: asking for the catalog
    // opens a session, and that should leave nothing in the user's workspace.
    let scratch = state.data_dir.join("provider-probe");
    tauri::async_runtime::spawn_blocking(move || match provider {
        Provider::Claude => claude::auth::read_models_for_account(&resolved.program, &resolved.prefix_args, &scratch, folder.as_deref(), Duration::from_secs(45)).map_err(DesktopError::io),
        Provider::Codex => codex::probe::read_models(&resolved.program, folder.as_deref(), Duration::from_secs(45)).map_err(DesktopError::io),
        Provider::Gemini => gemini::probe::read_models(&resolved.program, Duration::from_secs(45)).map_err(DesktopError::io),
        Provider::Local => {
            let _ = &resolved;
            thingmaker_supervisor::agents::local::ollama::Ollama::local().models().map(|models| thingmaker_supervisor::agents::local::catalog(&models)).map_err(DesktopError::not_ready)
        }
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?
}

/// Asks a provider for its account's quota now, rather than waiting for the
/// next report on a session's stream.
///
/// Reads each CLI’s own usage data without spending a model turn.
#[tauri::command]
pub async fn provider_quota(provider: Provider, state: State<'_, AppState>) -> CommandResult<thingmaker_supervisor::agents::events::QuotaSnapshot> {
    let resolved = state.resolve_provider(provider)?;
    let home = state.home.clone().unwrap_or_default();
    let snapshot = match provider {
        // Claude Code is asked for its own `/usage` data; the adapter's
        // script locates the Claude Code binary it ships with.
        Provider::Claude => {
            let script = resolved.prefix_args.first().map(std::path::PathBuf::from).unwrap_or_default();
            let binary = claude::usage_probe::locate_claude_binary(&script, &home)
                .ok_or_else(|| DesktopError::not_ready("Claude Code's own program was not found beside the adapter"))?;
            let folder = state.account_folder(Provider::Claude, None);
            tauri::async_runtime::spawn_blocking(move || claude::usage_probe::read_quota(&binary, folder.as_deref(), Duration::from_secs(30)).map_err(DesktopError::io))
                .await
                .map_err(|error| DesktopError::io(error.to_string()))??
        }
        Provider::Codex => {
            let folder = state.account_folder(Provider::Codex, None);
            tauri::async_runtime::spawn_blocking(move || codex::probe::read_quota(&resolved.program, folder.as_deref(), Duration::from_secs(30)).map_err(DesktopError::io))
                .await
                .map_err(|error| DesktopError::io(error.to_string()))??
        }
        Provider::Gemini => {
            tauri::async_runtime::spawn_blocking(move || gemini::quota::read_quota(&resolved.program, &home, Duration::from_secs(30)).map_err(DesktopError::io))
                .await
                .map_err(|error| DesktopError::io(error.to_string()))??
        }
        Provider::Local => return Err(DesktopError::unsupported("Local Models are free and have no quota.")),
    };
    // The router places work by the same reading the sidebar shows.
    if let Some(delegation) = state.delegation.get() {
        delegation.note_quota(snapshot.clone());
    }
    Ok(snapshot)
}

/// Signs a provider's program out of its own store, so the next sign-in can
/// be a different account. The provider's own command does it; nothing here
/// touches a credential file.
#[tauri::command]
pub async fn provider_logout(provider: Provider, account: Option<String>, state: State<'_, AppState>) -> CommandResult<()> {
    let resolved = state.resolve_provider(provider)?;
    let folder = state.account_folder(provider, account.as_deref());
    let (args, environment) = match provider {
        Provider::Claude => (claude::auth::logout_args(&resolved.prefix_args), claude_environment(folder.as_deref())),
        Provider::Codex => (codex::probe::logout_args(), codex::probe::environment(folder.as_deref()).with_program_dir(&resolved.program)),
        Provider::Gemini => {
            return Err(DesktopError::unsupported("Antigravity signs in and out through its own app: switch the account there, or run `agy` in a terminal."));
        }
        Provider::Local => return Err(DesktopError::unsupported("Local Models have no account to sign out of.")),
    };
    let program = resolved.program;
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = std::process::Command::new(&program);
        command.args(&args).env_clear().envs(environment.resolve(std::env::vars())).stdin(std::process::Stdio::null());
        let output = run_bounded(command, Duration::from_secs(30)).map_err(DesktopError::io)?;
        if output.status.success() {
            Ok(())
        } else {
            let detail = String::from_utf8_lossy(&output.stderr).trim().chars().take(400).collect::<String>();
            Err(DesktopError::io(format!("sign-out failed{}", if detail.is_empty() { String::new() } else { format!(": {detail}") })))
        }
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?
}

/// Opens a URL in the system browser after validation. Only http(s) URLs are
/// accepted; the user triggers this explicitly from a displayed URL (SEC-06).
#[tauri::command]
pub fn open_external(url: String, app: tauri::AppHandle) -> CommandResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let trimmed = url.trim();
    let allowed = trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1") || trimmed.starts_with("http://localhost");
    if !allowed || trimmed.len() > 4096 || trimmed.chars().any(char::is_control) {
        return Err(DesktopError::unsupported("Only https URLs (or loopback http) can be opened from the desktop."));
    }
    app.opener()
        .open_url(trimmed, None::<&str>)
        .map_err(|error| DesktopError::io(format!("could not open the browser: {error}")))
}
