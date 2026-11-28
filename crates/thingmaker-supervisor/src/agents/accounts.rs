//! More than one account per provider, switched without signing out.
//!
//! Claude Code keeps its sign-in per settings folder (`CLAUDE_CONFIG_DIR`),
//! and Codex under `CODEX_HOME`. Gemini uses a separate home and file-based
//! CLI authentication (see `gemini::launch::account_environment`).
//! Each extra account is a folder of its
//! own inside ThingMaker's data folder. Provider settings, MCP servers,
//! skills, plugins and conversation history are shared. Gemini uses a minimal
//! private home: arbitrary files tools create there are retained on removal.
//!
//! The person's own folders (`~/.claude`, `~/.codex`, `~/.gemini`) are the default
//! account. Gemini links existing configuration directories;
//! its credentials are never linked or copied.

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
    matches!(provider, Provider::Claude | Provider::Codex | Provider::Gemini)
}

/// The variable that points a provider's program at an account's folder.
pub fn folder_variable(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Claude => Some("CLAUDE_CONFIG_DIR"),
        Provider::Codex => Some("CODEX_HOME"),
        Provider::Copilot => Some("COPILOT_HOME"),
        Provider::Gemini | Provider::Local => None,
    }
}

/// The person's own folder for a provider: where the default account lives.
pub fn main_folder(provider: Provider, home: &Path) -> Option<PathBuf> {
    if provider == Provider::Gemini {
        return Some(home.join(".gemini"));
    }
    let variable = folder_variable(provider)?;
    let fallback = match provider {
        Provider::Claude => home.join(".claude"),
        Provider::Copilot => home.join(".copilot"),
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
/// in `folder`; Gemini can also publish a saved settings file to `main`.
pub fn share(provider: Provider, main: &Path, folder: &Path, home: &Path) -> std::io::Result<()> {
    if provider == Provider::Gemini {
        return share_gemini(main, folder, home);
    }
    // Copilot may use the OS keyring or gh auth outside COPILOT_HOME.
    // Folder isolation is not an established multi-account auth boundary.
    if provider == Provider::Copilot {
        return Err(std::io::Error::other("Copilot supports only its main account here; use copilot login"));
    }
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

/// Antigravity has no config-folder override for authentication. Each extra
/// account gets a minimal home containing its own .gemini credential files.
/// Never link the whole real home: Library/AppData can contain this profile.
/// Only the provider writes credentials; none are copied from the main account.
fn share_gemini(main: &Path, folder: &Path, home: &Path) -> std::io::Result<()> {
    let gemini = folder.join(".gemini");
    for path in [folder, &gemini] {
        if path.symlink_metadata().is_ok() && profile_link(path)? {
            return Err(std::io::Error::other("the account and credential directories must not be links"));
        }
    }
    std::fs::create_dir_all(&gemini)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&gemini, std::fs::Permissions::from_mode(0o700))?;
    }
    // Remove the broad home links made by earlier builds, without following
    // them or deleting files a tool created in this profile.
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_name() == ".gemini" { continue; }
        let expected = home.join(entry.file_name());
        let legacy = std::fs::read_link(entry.path()).ok().is_some_and(|target| target == expected)
            || std::fs::canonicalize(entry.path()).ok().zip(std::fs::canonicalize(&expected).ok()).is_some_and(|(actual, expected)| actual == expected);
        if legacy && profile_link(&entry.path())? { remove_profile_link(&entry.path())?; }
    }
    // Explicitly share known non-credential content. Token files and keyring
    // timeout markers stay private, including names introduced by future agy builds.
    for name in ["config", "skills", "commands"] {
        let source = main.join(name);
        let target = gemini.join(name);
        publish_private_directory(&target, &source)?;
        if !source.try_exists()? { continue; }
        if !source.is_dir() { return Err(std::io::Error::other(format!("expected a directory at {}", source.display()))); }
        // Old profiles contained a real config directory with only an MCP
        // link. Preserve other private configuration before replacing it.
        if target.is_dir() && !profile_link(&target)? {
            std::fs::rename(&target, folder.join(format!("gemini-{name}.before-sharing")))?;
        }
        if target.symlink_metadata().is_err() { gemini_link(&source, &target)?; }
    }
    // The normal CLI and ACP keep credentials INSIDE antigravity-cli. Sharing
    // that directory would share sign-ins too, even with a private HOME.
    let main_cli = main.join("antigravity-cli");
    let cli = gemini.join("antigravity-cli");
    if cli.symlink_metadata().is_ok() && profile_link(&cli)? { remove_profile_link(&cli)?; }
    std::fs::create_dir_all(&cli)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))?;
    }
    for name in ["conversations", "brain", "knowledge", "implicit", "mcp", "plugin_data", "builtin", "bin"] {
        let source = main_cli.join(name);
        let target = cli.join(name);
        if name == "conversations" { publish_private_directory(&target, &source)?; }
        if !source.is_dir() { continue; }
        if target.symlink_metadata().is_err() { gemini_link(&source, &target)?; }
    }
    for name in GEMINI_CLI_FILES {
        share_gemini_file(folder, &main_cli.join(name), &cli.join(name))?;
    }
    for name in GEMINI_HOME_FILES {
        share_gemini_file(folder, &main.join(name), &gemini.join(name))?;
    }
    Ok(())
}

// If the CLI created shared content first in the extra account, publish that
// populated directory on the next preparation without seeding empty main folders.
fn publish_private_directory(target: &Path, source: &Path) -> std::io::Result<()> {
    if !source.try_exists()? && target.is_dir() && !profile_link(target)? && std::fs::read_dir(target)?.next().transpose()?.is_some() {
        if let Some(parent) = source.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::rename(target, source)?;
    }
    Ok(())
}

fn preserve_conflict(folder: &Path, target: &Path) -> std::io::Result<()> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let backup = folder.join(format!("{name}.conflict-{stamp}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::rename(target, &backup)?;
    tracing::warn!(file = %backup.display(), "saved conflicting Gemini settings");
    Ok(())
}

pub fn gemini_conflicts(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder).into_iter().flatten().flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".conflict-"))
        .map(|entry| entry.path()).collect()
}

fn share_gemini_file(folder: &Path, source: &Path, target: &Path) -> std::io::Result<()> {
    // Resolve managed dotfiles before publishing by rename, including a dangling final destination.
    let mut destination = source.to_path_buf();
    for _ in 0..40 {
        if !destination.is_symlink() { break; }
        let link = std::fs::read_link(&destination)?;
        destination = destination.parent().unwrap_or(Path::new("")).join(link);
    }
    if destination.is_symlink() { return Err(std::io::Error::other("shared setting contains a symlink loop")); }
    let source = destination.as_path();
    let original = target.with_extension("thingmaker-shared-hash");
    if original.symlink_metadata().is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(std::io::Error::other("shared-settings metadata must not be a link"));
    }
    let baseline = std::fs::read_to_string(&original).ok();
    if !source.exists() && target.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
        if let Some(baseline) = baseline.as_deref() {
            if blake3::hash(&std::fs::read(target)?).to_hex().as_str() != baseline {
                preserve_conflict(folder, target)?;
            } else {
                std::fs::remove_file(target)?;
            }
            std::fs::remove_file(original)?;
            return Ok(());
        }
        if let Some(parent) = source.parent() { std::fs::create_dir_all(parent)?; }
        match std::fs::hard_link(target, source) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }
    if source.is_file() {
        let source_bytes = std::fs::read(source)?;
        let source_hash = blake3::hash(&source_bytes).to_hex().to_string();
        if let Ok(metadata) = target.symlink_metadata() {
            #[cfg(windows)]
            let shared = same_file::is_same_file(source, target).unwrap_or(false);
            #[cfg(not(windows))]
            let shared = metadata.file_type().is_symlink() && std::fs::read_link(target)?.as_path() == source;
            #[cfg(windows)]
            let _ = metadata;
            if !shared {
                if target.is_symlink() && !target.exists() {
                    std::fs::remove_file(target)?;
                    gemini_link(source, target)?;
                    return std::fs::write(original, source_hash);
                }
                let target_bytes = std::fs::read(target)?;
                let target_hash = blake3::hash(&target_bytes).to_hex().to_string();
                if target_hash != source_hash && baseline.as_deref() == Some(source_hash.as_str()) {
                    // The CLI saved by rename and only this profile changed.
                    // Preserve the main file's permissions and publish the save.
                    if std::fs::read(source)? != source_bytes { return Err(std::io::Error::other("shared settings changed during synchronization; try again")); }
                    let temp = source.with_extension(format!("thingmaker-{}.tmp", uuid::Uuid::new_v4().simple()));
                    let outcome = std::fs::write(&temp, &target_bytes)
                        .and_then(|()| std::fs::set_permissions(&temp, std::fs::metadata(source)?.permissions()))
                        .and_then(|()| std::fs::rename(&temp, source));
                    if let Err(error) = outcome { let _ = std::fs::remove_file(temp); return Err(error); }
                } else if target_hash != source_hash && baseline.as_deref() != Some(target_hash.as_str()) {
                    preserve_conflict(folder, target)?;
                }
                if target.symlink_metadata().is_ok() { std::fs::remove_file(target)?; }
                gemini_link(source, target)?;
            }
        } else { gemini_link(source, target)?; }
        std::fs::write(original, blake3::hash(&std::fs::read(source)?).to_hex().as_bytes())?;
    }
    Ok(())
}

const GEMINI_HOME_FILES: &[&str] = &["GEMINI.md", "settings.json", "trustedFolders.json", "projects.json"];
const GEMINI_CLI_FILES: &[&str] = &["settings.json", "keybindings.json"];

/// Remove credentials and provider links, retaining files and links tools wrote in `~`.
pub fn remove_gemini_profile(folder: &Path) -> std::io::Result<()> {
    if folder.symlink_metadata().is_ok() && profile_link(folder)? {
        return Err(std::io::Error::other("account directory must not be a link"));
    }
    let gemini = folder.join(".gemini");
    if gemini.symlink_metadata().is_ok() && profile_link(&gemini)? {
        remove_profile_link(&gemini)?;
    } else if gemini.exists() {
        // Newly initialized accounts may have written content before the main
        // CLI created a matching directory. Keep those files outside the profile.
        for (root, names) in [(&gemini, &["config", "skills", "commands"][..]), (&gemini.join("antigravity-cli"), &["conversations", "brain", "knowledge", "implicit", "mcp", "plugin_data"][..])] {
            if !root.is_dir() || profile_link(root)? { continue; }
            for name in names {
                let child = root.join(name);
                if child.is_dir() && !profile_link(&child)? && std::fs::read_dir(&child)?.next().transpose()?.is_some() {
                    std::fs::rename(&child, folder.join(format!("gemini-{name}.saved-{}", uuid::Uuid::new_v4().simple())))?;
                }
            }
        }
        // Atomic saves can replace a shared file between the last preparation and removal.
        // Keep independent settings outside the credential tree, even when logout/sync failed.
        for (root, names) in [(&gemini, GEMINI_HOME_FILES), (&gemini.join("antigravity-cli"), GEMINI_CLI_FILES)] {
            if !root.is_dir() || profile_link(root)? { continue; }
            for name in names {
                let path = root.join(name);
                match path.symlink_metadata() {
                    Ok(meta) if meta.is_file() => {
                        let baseline = path.with_extension("thingmaker-shared-hash");
                        let unchanged = baseline.symlink_metadata().is_ok_and(|m| m.is_file()) &&
                            std::fs::read_to_string(baseline).ok().is_some_and(|hash|
                                std::fs::read(&path).is_ok_and(|bytes| blake3::hash(&bytes).to_hex().as_str() == hash));
                        if !unchanged {
                            std::fs::rename(path, folder.join(format!("gemini-{name}.saved-{}", uuid::Uuid::new_v4().simple())))?;
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        std::fs::remove_dir_all(gemini)?;
    }
    if folder.is_dir() && std::fs::read_dir(folder)?.next().is_none() { std::fs::remove_dir(folder)?; }
    Ok(())
}

fn profile_link(path: &Path) -> std::io::Result<bool> {
    #[cfg(windows)]
    if is_junction(path)? { return Ok(true); }
    Ok(path.symlink_metadata()?.file_type().is_symlink())
}

#[cfg(windows)]
pub(crate) fn is_junction(path: &Path) -> std::io::Result<bool> {
    match junction::exists(path) {
        Ok(value) => Ok(value),
        Err(error) if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_NOT_A_REPARSE_POINT as i32) => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_profile_link(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if is_junction(path)? {
            junction::delete(path)?;
            return std::fs::remove_dir(path);
        }
        if path.is_dir() { return std::fs::remove_dir(path); }
    }
    std::fs::remove_file(path)
}

fn gemini_link(source: &Path, target: &Path) -> std::io::Result<()> {
    // Junctions and hard links work without Windows Developer Mode. Directory
    // links also keep write-then-rename updates in the shared configuration.
    #[cfg(windows)]
    { if source.is_dir() { junction::create(source, target) } else { std::fs::hard_link(source, target) } }
    #[cfg(not(windows))]
    { link(source, target) }
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

    #[test]
    fn copilot_maps_configuration_but_does_not_claim_folder_auth_isolation() {
        assert_eq!(folder_variable(Provider::Copilot), Some("COPILOT_HOME"));
        assert!(!supports_accounts(Provider::Copilot));
        let directory = tempfile::tempdir().unwrap();
        assert!(share(Provider::Copilot, directory.path(), &directory.path().join("extra"), directory.path()).is_err());
        assert!(!directory.path().join("extra").exists());
    }
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
    fn gemini_homes_share_settings_and_history_but_never_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let main = home.join(".gemini");
        std::fs::create_dir_all(main.join("antigravity-cli/conversations")).unwrap();
        std::fs::create_dir_all(main.join("config")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(main.join("config/mcp_config.json"), "{}").unwrap();
        std::fs::write(main.join("antigravity-oauth-token"), "main-secret").unwrap();
        std::fs::write(main.join("future-credential-file"), "main-secret").unwrap();
        std::fs::write(main.join("antigravity-cli/antigravity-oauth-token"), "main-cli-secret").unwrap();
        std::fs::write(main.join("antigravity-cli/future-credential-file"), "main-cli-secret").unwrap();
        let work = dir.path().join("accounts/gemini/work");
        let personal = dir.path().join("accounts/gemini/personal");
        for folder in [&work, &personal] {
            share(Provider::Gemini, &main, folder, &home).unwrap();
            assert!(folder.join(".gemini/antigravity-cli/conversations").is_dir());
            assert!(!profile_link(&folder.join(".gemini/antigravity-cli")).unwrap(), "the CLI root contains credentials and must be private");
            assert!(!folder.join(".gemini/antigravity-cli/antigravity-oauth-token").exists());
            assert!(!folder.join(".gemini/antigravity-cli/future-credential-file").exists());
            assert!(profile_link(&folder.join(".gemini/config")).unwrap());
            #[cfg(windows)]
            assert!(junction::exists(folder.join(".gemini/config")).unwrap(), "Gemini must use junctions rather than privileged directory symlinks");
            assert!(folder.join(".gemini/config/mcp_config.json").is_file());
            assert!(!folder.join(".ssh").exists(), "the real home is not mirrored");
            assert!(!folder.join(".gemini/antigravity-oauth-token").exists());
            assert!(!folder.join(".gemini/future-credential-file").exists());
        }
        std::fs::write(work.join(".gemini/antigravity-oauth-token"), "work-secret").unwrap();
        share(Provider::Gemini, &main, &work, &home).unwrap();
        assert_eq!(std::fs::read_to_string(work.join(".gemini/antigravity-oauth-token")).unwrap(), "work-secret");
        assert_eq!(std::fs::read_to_string(main.join("antigravity-oauth-token")).unwrap(), "main-secret");
        std::fs::remove_dir_all(work).unwrap();
        assert!(main.join("antigravity-cli/conversations").is_dir());
        assert!(!personal.join(".gemini/antigravity-oauth-token").exists());
    }

    #[test]
    fn gemini_configuration_survives_atomic_saves_and_profile_removal_keeps_user_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let main = home.join(".gemini");
        let work = home.join("Library/ThingMaker/accounts/work");
        let personal = home.join("Library/ThingMaker/accounts/personal");
        std::fs::create_dir_all(main.join("antigravity-cli/conversations")).unwrap();
        std::fs::write(main.join("antigravity-cli/conversations/kept.json"), "conversation").unwrap();
        std::fs::create_dir_all(main.join("config/plugins")).unwrap();
        std::fs::write(main.join("trustedFolders.json"), "{}").unwrap();
        std::fs::write(main.join("projects.json"), "{}").unwrap();
        for folder in [&work, &personal] { share(Provider::Gemini, &main, folder, &home).unwrap(); }
        // A legacy whole-home link included this profile itself.
        gemini_link(&home.join("Library"), &work.join("Library")).unwrap();
        share(Provider::Gemini, &main, &work, &home).unwrap();
        assert!(!work.join("Library").exists());
        for name in ["mcp_config.json", "config.json"] {
            let target = work.join(".gemini/config").join(name);
            std::fs::write(&target, "old").unwrap();
            let temp = target.with_extension("tmp");
            std::fs::write(&temp, "updated").unwrap();
            std::fs::rename(temp, target).unwrap();
            assert_eq!(std::fs::read_to_string(personal.join(".gemini/config").join(name)).unwrap(), "updated");
        }
        assert!(work.join(".gemini/config/plugins").is_dir());
        assert!(work.join(".gemini/trustedFolders.json").is_file());
        assert!(work.join(".gemini/projects.json").is_file());
        std::fs::write(work.join("result.txt"), "user work").unwrap();
        let results = dir.path().join("user-results");
        std::fs::create_dir(&results).unwrap();
        gemini_link(&results, &work.join("outputs")).unwrap();
        share(Provider::Gemini, &main, &work, &home).unwrap();
        assert!(work.join("outputs").is_dir(), "user-created links must survive profile preparation");
        std::fs::write(work.join(".gemini/jetski-standalone-oauth-token"), "private").unwrap();
        remove_gemini_profile(&work).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("result.txt")).unwrap(), "user work");
        assert!(work.join("outputs").is_dir(), "user-created links must survive account removal");
        assert!(!work.join(".gemini").exists());
        assert_eq!(std::fs::read_to_string(main.join("antigravity-cli/conversations/kept.json")).unwrap(), "conversation");
        assert!(!main.join("antigravity-cli/bin").exists());
        assert!(main.join("config/config.json").is_file());
        assert!(personal.join(".gemini/config/config.json").is_file());
    }

    #[test]
    fn a_replaced_shared_file_is_reported_instead_of_silently_diverging() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("settings.json"), "original").unwrap();
        let folder = dir.path().join("account");
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        let target = folder.join(".gemini/settings.json");
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, "private update").unwrap();
        std::fs::write(main.join("settings.json"), "main update").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(main.join("settings.json")).unwrap(), "main update");
        assert_eq!(std::fs::read_to_string(target).unwrap(), "main update");
        let backups = gemini_conflicts(&folder);
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "private update");
    }

    #[cfg(unix)]
    #[test]
    fn profile_saves_preserve_main_dotfile_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join(".gemini");
        let folder = dir.path().join("account");
        std::fs::create_dir_all(&main).unwrap();
        let managed = dir.path().join("managed");
        std::fs::write(&managed, "original").unwrap();
        std::os::unix::fs::symlink("../managed", main.join("settings.json")).unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        let local = folder.join(".gemini/settings.json");
        std::fs::remove_file(&local).unwrap();
        std::fs::write(&local, "updated").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert!(main.join("settings.json").is_symlink());
        assert_eq!(std::fs::read_to_string(managed).unwrap(), "updated");
    }

    #[test]
    fn gemini_atomic_shared_preferences_are_synchronized_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        let folder = dir.path().join("account");
        std::fs::create_dir_all(main.join("antigravity-cli")).unwrap();
        let source = main.join("antigravity-cli/settings.json");
        std::fs::write(&source, "original").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        let target = folder.join(".gemini/antigravity-cli/settings.json");
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, "profile save").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "profile save");
        let temp = source.with_extension("tmp");
        std::fs::write(&temp, "main save").unwrap();
        std::fs::rename(temp, &source).unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(target).unwrap(), "main save");
    }

    #[test]
    fn an_added_accounts_first_conversation_is_shared_without_creating_empty_cli_folders() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        let folder = dir.path().join("account");
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert!(!main.exists());
        let history = folder.join(".gemini/antigravity-cli/conversations");
        std::fs::create_dir_all(&history).unwrap();
        std::fs::write(history.join("first"), "content").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        remove_gemini_profile(&folder).unwrap();
        assert_eq!(std::fs::read_to_string(main.join("antigravity-cli/conversations/first")).unwrap(), "content");
        assert!(!main.join("antigravity-cli/bin").exists());
    }

    #[test]
    fn gemini_new_shared_files_and_main_resets_do_not_leave_private_copies() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        let folder = dir.path().join("account");
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        let source = main.join("projects.json");
        let target = folder.join(".gemini/projects.json");
        std::fs::write(&target, "created by the CLI").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "created by the CLI");
        std::fs::remove_file(&source).unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert!(!source.exists());
        assert!(!target.exists());
    }

    #[test]
    fn legacy_private_configuration_is_preserved_outside_the_removable_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        let folder = dir.path().join("account");
        std::fs::create_dir_all(main.join("config")).unwrap();
        std::fs::create_dir_all(folder.join(".gemini/config/plugins")).unwrap();
        std::fs::write(folder.join(".gemini/config/config.json"), "private preferences").unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert!(profile_link(&folder.join(".gemini/config")).unwrap());
        let preserved = folder.join("gemini-config.before-sharing/config.json");
        assert_eq!(std::fs::read_to_string(&preserved).unwrap(), "private preferences");
        remove_gemini_profile(&folder).unwrap();
        assert_eq!(std::fs::read_to_string(preserved).unwrap(), "private preferences");
        assert!(main.join("config").is_dir());
    }

    #[test]
    fn a_legacy_shared_cli_directory_is_replaced_without_copying_its_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("home/.gemini");
        let folder = dir.path().join("account");
        std::fs::create_dir_all(main.join("antigravity-cli/conversations")).unwrap();
        std::fs::create_dir_all(folder.join(".gemini")).unwrap();
        std::fs::write(main.join("antigravity-cli/antigravity-oauth-token"), "never share").unwrap();
        gemini_link(&main.join("antigravity-cli"), &folder.join(".gemini/antigravity-cli")).unwrap();
        share(Provider::Gemini, &main, &folder, dir.path()).unwrap();
        assert!(!profile_link(&folder.join(".gemini/antigravity-cli")).unwrap());
        assert!(!folder.join(".gemini/antigravity-cli/antigravity-oauth-token").exists());
        assert!(folder.join(".gemini/antigravity-cli/conversations").is_dir());
        assert_eq!(std::fs::read_to_string(main.join("antigravity-cli/antigravity-oauth-token")).unwrap(), "never share");
    }

    #[test]
    fn stored_accounts_are_read_safely() {
        let value = json!([
            { "id": "work-1", "provider": "claude", "label": "Work" },
            { "id": "../evil", "provider": "codex", "label": "x" },
            { "id": "g", "provider": "gemini", "label": "Personal" },
            { "id": "default", "provider": "claude", "label": "dup" }
        ]);
        let accounts = read_accounts(Some(value));
        assert_eq!(accounts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["work-1", "g"]);
    }
}
