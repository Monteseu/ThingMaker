//! More than one account per provider (Claude Code, Codex, Gemini): list, add,
//! rename, remove, and pick the active one. See `agents::accounts`.
//!
//! The active account applies to sessions started from then on, and to the
//! provider's sign-in status and usage. Signing in to an account is the
//! ordinary sign-in run inside its folder (`provider_login_start` with
//! `account`).

use serde::Serialize;
use tauri::State;
use thingmaker_supervisor::{
    agents::{
        Provider,
        accounts::{self, Account, DEFAULT_ACCOUNT},
        auth::ProviderAuthStatus,
    },
    error::DesktopError,
    storage::settings::GLOBAL_SCOPE,
};

use super::CommandResult;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    /// The person's own folder (`~/.claude`, `~/.codex`).
    pub is_default: bool,
    pub active: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuthStatus>,
}

fn storage_error(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::io(error.to_string())
}

fn check(provider: Provider) -> CommandResult<()> {
    if accounts::supports_accounts(provider) { Ok(()) } else { Err(DesktopError::unsupported(format!("{} has one account here", provider.label()))) }
}

fn save(state: &AppState, list: &[Account]) -> CommandResult<()> {
    state.with_storage(|s| s.setting_set(accounts::ACCOUNTS_SETTING, GLOBAL_SCOPE, &list)).map_err(storage_error)
}

/// A provider's accounts, the default first, each with its sign-in.
#[tauri::command]
pub async fn accounts_list(provider: Provider, state: State<'_, AppState>) -> CommandResult<Vec<AccountView>> {
    // Display the main account and its usage without offering unsafe isolation.
    if provider != Provider::Copilot { check(provider)?; }
    let active = state.active_account(provider);
    let mut views = vec![AccountView { id: DEFAULT_ACCOUNT.into(), provider, label: String::new(), is_default: true, active: active == DEFAULT_ACCOUNT, auth: None, conflicts: Vec::new() }];
    views.extend(state.accounts().into_iter().filter(|account| account.provider == provider).map(|account| AccountView { active: active == account.id, is_default: false, auth: None, conflicts: Vec::new(), id: account.id, provider, label: account.label }));
    let resolved = match state.resolve_provider(provider) {
        Ok(resolved) => resolved,
        Err(_) => return Ok(views),
    };
    let folders: Vec<_> = views.iter().map(|view| state.try_account_folder(provider, Some(&view.id))).collect();
    if provider == Provider::Gemini {
        for (view, folder) in views.iter_mut().zip(&folders) {
            if let Ok(Some(folder)) = folder { view.conflicts = accounts::gemini_conflicts(folder); }
        }
    }
    let gemini_accounts = views.len() > 1;
    // Each answers through its own program run; asked side by side.
    let statuses = tauri::async_runtime::spawn_blocking(move || {
        let threads: Vec<_> = folders
            .into_iter()
            .map(|folder| {
                let resolved = resolved.clone();
                std::thread::spawn(move || match folder {
                    Ok(folder) => super::providers::auth_status(&resolved, folder.as_deref(), gemini_accounts),
                    Err(error) => ProviderAuthStatus { setup_required: None, provider, logged_in: Some(false), method: None, account: None, plan: None, problem: Some(error.message) },
                })
            })
            .collect();
        threads.into_iter().map(|thread| thread.join().ok()).collect::<Vec<_>>()
    })
    .await
    .map_err(|error| DesktopError::io(error.to_string()))?;
    for (view, status) in views.iter_mut().zip(statuses) {
        view.auth = status;
    }
    Ok(views)
}

/// Adds an account with its own folder, sharing the main one but the
/// sign-in. Signing in to it is the next step.
#[tauri::command]
pub fn account_add(provider: Provider, label: String, state: State<'_, AppState>) -> CommandResult<AccountView> {
    check(provider)?;
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err(DesktopError::unsupported("give the account a name"));
    }
    let id = format!("acct-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let previous = state.accounts();
    let mut list = previous.clone();
    list.push(Account { id: id.clone(), provider, label: label.clone() });
    save(&state, &list)?;
    if let Err(error) = state.try_account_folder(provider, Some(&id)) {
        save(&state, &previous)?;
        return Err(error);
    }
    Ok(AccountView { id, provider, label, is_default: false, active: false, auth: None, conflicts: Vec::new() })
}

#[tauri::command]
pub fn account_rename(provider: Provider, id: String, label: String, state: State<'_, AppState>) -> CommandResult<()> {
    check(provider)?;
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err(DesktopError::unsupported("give the account a name"));
    }
    let mut list = state.accounts();
    let account = list.iter_mut().find(|account| account.provider == provider && account.id == id).ok_or_else(|| DesktopError::not_ready("that account is gone"))?;
    account.label = label;
    save(&state, &list)
}

/// Removes an added account: signs it out and deletes its folder. Its links
/// go with it; the main folder they point to is untouched. The default
/// account cannot be removed.
#[tauri::command]
pub async fn account_remove(provider: Provider, id: String, local_only: Option<bool>, state: State<'_, AppState>) -> CommandResult<Option<String>> {
    check(provider)?;
    if id == DEFAULT_ACCOUNT || !accounts::valid_id(&id) {
        return Err(DesktopError::unsupported("your own account cannot be removed here"));
    }
    let folder = accounts::account_folder(&state.data_dir, provider, &id);
    if provider == Provider::Gemini {
        if !state.accounts().iter().any(|a| a.provider == provider && a.id == id) {
            return Err(DesktopError::not_ready("that account is gone"));
        }
        let result = remove_gemini(&folder, local_only.unwrap_or(false), async {
            let resolved = state.resolve_provider(provider).map_err(|e| e.message)?;
            thingmaker_supervisor::agents::gemini::probe::logout_for_account(&resolved.program, &folder, std::time::Duration::from_secs(15)).await
        }).await?;
        if result.is_some() { return Ok(result); }
    } else {
        let _ = super::providers::provider_logout(provider, Some(id.clone()), state.clone()).await;
        let _ = std::fs::remove_dir_all(&folder);
    }
    let list: Vec<Account> = state.accounts().into_iter().filter(|account| !(account.provider == provider && account.id == id)).collect();
    save(&state, &list)?;
    if state.active_account(provider) == DEFAULT_ACCOUNT {
        let _ = state.with_storage(|s| s.setting_set(accounts::ACTIVE_ACCOUNT_SETTING, provider.as_str(), &DEFAULT_ACCOUNT));
    }
    Ok(None)
}

async fn remove_gemini(folder: &std::path::Path, local_only: bool, logout: impl Future<Output = Result<(), String>>) -> CommandResult<Option<String>> {
    use thingmaker_supervisor::agents::gemini::launch::{token_path, legacy_token_path};
    if !local_only && (token_path(folder).try_exists().map_err(storage_error)? || legacy_token_path(folder).try_exists().map_err(storage_error)?) {
        if let Err(reason) = logout.await { return Ok(Some(reason)); }
        if token_path(folder).try_exists().map_err(storage_error)? || legacy_token_path(folder).try_exists().map_err(storage_error)? {
            return Ok(Some("Antigravity kept the local sign-in token.".into()));
        }
    }
    accounts::remove_gemini_profile(folder).map_err(storage_error)?;
    Ok(None)
}

/// Makes an account the one new sessions, status and usage use.
#[tauri::command]
pub fn account_set_active(provider: Provider, id: String, state: State<'_, AppState>) -> CommandResult<()> {
    check(provider)?;
    if id != DEFAULT_ACCOUNT && !state.accounts().iter().any(|account| account.provider == provider && account.id == id) {
        return Err(DesktopError::not_ready("that account is gone"));
    }
    state.try_account_folder(provider, Some(&id))?;
    state.with_storage(|s| s.setting_set(accounts::ACTIVE_ACCOUNT_SETTING, provider.as_str(), &id)).map_err(storage_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_removal_handles_failed_logout_changed_cli_and_conflicting_settings() {
        for reason in ["revoked grant", "unverified CLI version"] {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("account");
            let main = dir.path().join("home/.gemini");
            std::fs::create_dir_all(&main).unwrap();
            std::fs::write(main.join("settings.json"), "baseline").unwrap();
            accounts::share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
            let token = thingmaker_supervisor::agents::gemini::launch::token_path(&folder);
            std::fs::write(&token, "fixture").unwrap();
            let settings = folder.join(".gemini/settings.json");
            std::fs::remove_file(&settings).unwrap();
            std::fs::write(settings, "unshared local change").unwrap();
            std::fs::write(main.join("settings.json"), "main change").unwrap();
            assert_eq!(remove_gemini(&folder, false, async { Err(reason.to_string()) }).await.unwrap().as_deref(), Some(reason));
            assert!(token.exists());
            assert!(remove_gemini(&folder, true, async { panic!("local removal cannot require a working CLI") }).await.unwrap().is_none());
            assert!(!token.exists());
            let saved = std::fs::read_dir(&folder).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("gemini-settings.json.saved-")).unwrap();
            assert_eq!(std::fs::read_to_string(saved.path()).unwrap(), "unshared local change");
            assert_eq!(std::fs::read_to_string(main.join("settings.json")).unwrap(), "main change");
        }
    }
}
