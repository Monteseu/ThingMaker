//! More than one account per provider, switched without signing out.
//!
//! Claude Code keeps its sign-in per settings folder (`CLAUDE_CONFIG_DIR`),
//! and Codex under `CODEX_HOME`. So each extra account is a folder of its
//! own inside ThingMaker's data folder, where only the sign-in is the
//! account's: everything else (settings, MCP servers, skills, plugins,
//! memory, conversation history) is a link back to the person's main folder,
//! so switching accounts changes who is signed in and nothing else, as
//! signing out and in again did.
//!
//! The person's own folders (`~/.claude`, `~/.codex`) are the default
//! account and are never changed here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Provider;

/// The default account's id: the person's own folder.
pub const DEFAULT_ACCOUNT: &str = "default";

/// The setting that lists the accounts (global scope).
pub const ACCOUNTS_SETTING: &str = "accounts";
/// The setting that names each provider's active account (scope: provider).
pub const ACTIVE_ACCOUNT_SETTING: &str = "activeAccount";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    /// What the person calls it: "Work", "Personal".
    pub label: String,
}

/// Whether a provider can have more than one account here.
pub fn supports_accounts(provider: Provider) -> bool {
    matches!(provider, Provider::Claude | Provider::Codex)
}

/// The variable that points a provider's program at an account's folder.
pub fn folder_variable(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Claude => Some("CLAUDE_CONFIG_DIR"),
        Provider::Codex => Some("CODEX_HOME"),
        Provider::Gemini | Provider::Local => None,
    }
}

/// The person's own folder for a provider: where the default account lives.
pub fn main_folder(provider: Provider, home: &Path) -> Option<PathBuf> {
    let variable = folder_variable(provider)?;
    let fallback = match provider {
        Provider::Claude => home.join(".claude"),
        _ => home.join(".codex"),
    };
    Some(std::env::var_os(variable).map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or(fallback))
}

/// An extra account's folder.
pub fn account_folder(data_dir: &Path, provider: Provider, id: &str) -> PathBuf {
    data_dir.join("accounts").join(provider.as_str()).join(id)
}

/// Whether an id names an account folder (and nothing outside it).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// What stays the account's own in its folder: the sign-in, and for Claude
/// Code the file that also holds the account's identity (merged, see
/// [`merge_claude_json`]).
fn own(provider: Provider, name: &str) -> bool {
    match provider {
        Provider::Claude => matches!(name, ".credentials.json" | ".claude.json" | ".claude.json.backup"),
        Provider::Codex => matches!(name, "auth.json"),
        _ => true,
    }
}

/// Keys of Claude Code's `.claude.json` that belong to the signed-in
/// account; everything else (MCP servers, projects, preferences) is shared.
const CLAUDE_IDENTITY_KEYS: &[&str] = &["oauthAccount", "userID", "primaryApiKey", "customApiKeyResponses"];

/// Makes `folder` an account folder sharing everything in `main` but the
/// sign-in. Called before each use, so what was added to the main folder
/// since is shared too. Only ever creates links and the account's own files
/// in `folder`; `main` is read, never changed.
pub fn share(provider: Provider, main: &Path, folder: &Path, home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    if main.is_dir() {
        for entry in std::fs::read_dir(main)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(text) = name.to_str() else { continue };
            if own(provider, text) {
                continue;
            }
            let target = folder.join(&name);
            if target.symlink_metadata().is_ok() {
                continue;
            }
            link(&entry.path(), &target)?;
        }
    }
    if provider == Provider::Claude {
        // With CLAUDE_CONFIG_DIR unset, Claude Code keeps this file in the home
        // folder rather than in ~/.claude.
        let shared = [main.join(".claude.json"), home.join(".claude.json")].into_iter().find(|path| path.is_file());
        let own_path = folder.join(".claude.json");
        let shared_value = shared.and_then(|path| read_json(&path)).unwrap_or(Value::Object(Default::default()));
        let own_value = read_json(&own_path);
        let merged = merge_claude_json(&shared_value, own_value.as_ref());
        let text = serde_json::to_string_pretty(&merged).map_err(std::io::Error::other)?;
        let temp = folder.join(".claude.json.thingmaker-tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, &own_path)?;
    }
    Ok(())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The main `.claude.json` with the account's own identity in place of the
/// main account's: shared MCP servers and projects, separate sign-in.
pub fn merge_claude_json(shared: &Value, own: Option<&Value>) -> Value {
    let mut merged = shared.as_object().cloned().unwrap_or_default();
    for key in CLAUDE_IDENTITY_KEYS {
        merged.remove(*key);
        if let Some(value) = own.and_then(|own| own.get(*key)) {
            merged.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(merged)
}

#[cfg(unix)]
fn link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn link(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() { std::os::windows::fs::symlink_dir(source, target) } else { std::os::windows::fs::symlink_file(source, target) }
}

/// Reads the account list from the stored setting, keeping only accounts
/// this build can use.
pub fn read_accounts(value: Option<Value>) -> Vec<Account> {
    value
        .and_then(|value| serde_json::from_value::<Vec<Account>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|account| supports_accounts(account.provider) && valid_id(&account.id) && account.id != DEFAULT_ACCOUNT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_account_folder_shares_everything_but_the_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let main = home.join(".claude");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::write(main.join("settings.json"), "{}").unwrap();
        std::fs::write(main.join(".credentials.json"), "secret").unwrap();
        std::fs::write(home.join(".claude.json"), json!({ "oauthAccount": { "emailAddress": "main" }, "userID": "u-main", "mcpServers": { "files": {} } }).to_string()).unwrap();
        let folder = dir.path().join("accounts/claude/work");
        share(Provider::Claude, &main, &folder, &home).unwrap();
        assert!(folder.join("skills").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(folder.join("settings.json").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!folder.join(".credentials.json").exists(), "the main sign-in is never shared");
        let own = read_json(&folder.join(".claude.json")).unwrap();
        assert_eq!(own["mcpServers"], json!({ "files": {} }), "MCP servers are shared");
        assert!(own.get("oauthAccount").is_none(), "the main account's identity is not");

        // After signing in, the account's identity stays its own when shared again.
        std::fs::write(folder.join(".claude.json"), json!({ "oauthAccount": { "emailAddress": "work" }, "userID": "u-work" }).to_string()).unwrap();
        std::fs::create_dir_all(main.join("plugins")).unwrap();
        share(Provider::Claude, &main, &folder, &home).unwrap();
        let own = read_json(&folder.join(".claude.json")).unwrap();
        assert_eq!(own["oauthAccount"]["emailAddress"], json!("work"));
        assert_eq!(own["mcpServers"], json!({ "files": {} }));
        assert!(folder.join("plugins").exists(), "what the main folder gains is shared too");
        assert_eq!(std::fs::read_to_string(main.join(".credentials.json")).unwrap(), "secret", "the main folder is untouched");
    }

    #[test]
    fn a_codex_account_keeps_only_its_auth_file() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join(".codex");
        std::fs::create_dir_all(main.join("sessions")).unwrap();
        std::fs::write(main.join("config.toml"), "").unwrap();
        std::fs::write(main.join("auth.json"), "{}").unwrap();
        let folder = dir.path().join("accounts/codex/personal");
        share(Provider::Codex, &main, &folder, dir.path()).unwrap();
        assert!(folder.join("config.toml").exists() && folder.join("sessions").exists());
        assert!(!folder.join("auth.json").exists());
    }

    #[test]
    fn stored_accounts_are_read_safely() {
        let value = json!([
            { "id": "work-1", "provider": "claude", "label": "Work" },
            { "id": "../evil", "provider": "codex", "label": "x" },
            { "id": "g", "provider": "gemini", "label": "no" },
            { "id": "default", "provider": "claude", "label": "dup" }
        ]);
        let accounts = read_accounts(Some(value));
        assert_eq!(accounts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["work-1"]);
    }
}
