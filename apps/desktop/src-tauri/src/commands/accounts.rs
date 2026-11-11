//! More than one account per provider (Claude Code, Codex): list, add,
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
    check(provider)?;
    let active = state.active_account(provider);
    let mut views = vec![AccountView { id: DEFAULT_ACCOUNT.into(), provider, label: String::new(), is_default: true, active: active == DEFAULT_ACCOUNT, auth: None }];
    views.extend(state.accounts().into_iter().filter(|account| account.provider == provider).map(|account| AccountView { active: active == account.id, is_default: false, auth: None, id: account.id, provider, label: account.label }));
    let resolved = match state.resolve_provider(provider) {
        Ok(resolved) => resolved,
        Err(_) => return Ok(views),
    };
    let folders: Vec<Option<std::path::PathBuf>> = views.iter().map(|view| state.account_folder(provider, Some(&view.id))).collect();
    // Each answers through its own program run; asked side by side.
    let statuses = tauri::async_runtime::spawn_blocking(move || {
        let threads: Vec<_> = folders
            .into_iter()
            .map(|folder| {
                let resolved = resolved.clone();
                std::thread::spawn(move || super::providers::auth_status(&resolved, folder.as_deref()))
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
    let mut list = state.accounts();
    list.push(Account { id: id.clone(), provider, label: label.clone() });
    save(&state, &list)?;
    state.account_folder(provider, Some(&id));
    Ok(AccountView { id, provider, label, is_default: false, active: false, auth: None })
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
pub async fn account_remove(provider: Provider, id: String, state: State<'_, AppState>) -> CommandResult<()> {
    check(provider)?;
    if id == DEFAULT_ACCOUNT || !accounts::valid_id(&id) {
        return Err(DesktopError::unsupported("your own account cannot be removed here"));
    }
    let _ = super::providers::provider_logout(provider, Some(id.clone()), state.clone()).await;
    let folder = accounts::account_folder(&state.data_dir, provider, &id);
    // Removes links, never what they point to.
    let _ = std::fs::remove_dir_all(&folder);
    let list: Vec<Account> = state.accounts().into_iter().filter(|account| !(account.provider == provider && account.id == id)).collect();
    save(&state, &list)?;
    if state.active_account(provider) == DEFAULT_ACCOUNT {
        let _ = state.with_storage(|s| s.setting_set(accounts::ACTIVE_ACCOUNT_SETTING, provider.as_str(), &DEFAULT_ACCOUNT));
    }
    Ok(())
}

/// Makes an account the one new sessions, status and usage use.
#[tauri::command]
pub fn account_set_active(provider: Provider, id: String, state: State<'_, AppState>) -> CommandResult<()> {
    check(provider)?;
    if id != DEFAULT_ACCOUNT && !state.accounts().iter().any(|account| account.provider == provider && account.id == id) {
        return Err(DesktopError::not_ready("that account is gone"));
    }
    state.with_storage(|s| s.setting_set(accounts::ACTIVE_ACCOUNT_SETTING, provider.as_str(), &id)).map_err(storage_error)?;
    state.account_folder(provider, Some(&id));
    Ok(())
}
