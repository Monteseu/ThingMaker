//! One-shot questions to `agy`: whether it is signed in, and which models
//! the account can select. `agy models` answers both and spends no tokens.
//!
//! `agy` has no status command and no headless sign-in: it shares its
//! sign-in with the Antigravity desktop app, or signs in when run once in a
//! terminal. The desktop never sees a credential.

use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    agents::{
        Provider,
        auth::{ProviderAuthStatus, ProviderModel},
    },
};

/// Where `agy` publishes its releases, one manifest per platform, each a
/// JSON object with `version`, `url` and `sha512`. This is the same host and
/// path the official installer reads (`antigravity.google/cli/install.sh`),
/// and the request is an anonymous GET: no process, so no console window,
/// which is the whole reason the desktop asks here rather than running the
/// CLI (`launch::DISABLE_AUTO_UPDATE`).
const RELEASES: &str = "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests";

/// This computer's manifest name. Every platform's manifest carries the same
/// release, but asking for our own keeps the answer true if that stops being
/// so.
fn platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows_amd64",
        ("windows", "aarch64") => "windows_arm64",
        ("macos", "aarch64") => "darwin_arm64",
        ("macos", "x86_64") => "darwin_amd64",
        ("linux", "x86_64") => "linux_amd64",
        ("linux", "aarch64") => "linux_arm64",
        _ => return None,
    })
}

/// The newest `agy` released for this computer. Read-only: it downloads
/// nothing and updates nothing.
pub fn latest_version(timeout: Duration) -> Result<String, String> {
    let platform = platform().ok_or("Antigravity publishes no build for this platform")?;
    let client = reqwest::blocking::Client::builder().timeout(timeout).build().map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{RELEASES}/{platform}.json"))
        .send()
        .map_err(|error| format!("could not reach the Antigravity release server: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("the Antigravity release server answered {}", response.status()));
    }
    let manifest: serde_json::Value = response.json().map_err(|error| format!("could not read the release manifest: {error}"))?;
    manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "the release manifest named no version".into())
}

/// What to tell someone who is not signed in.
pub const SIGN_IN_HINT: &str = "Sign in through the Antigravity app, or run `agy` once in a terminal and follow its sign-in.";

/// `agy models`, with a bound, in the documented environment.
pub fn read_models(program: &Path, timeout: Duration) -> Result<Vec<ProviderModel>, String> {
    read_models_for_account(program, None, timeout)
}

pub fn read_models_for_account(program: &Path, folder: Option<&Path>, timeout: Duration) -> Result<Vec<ProviderModel>, String> {
    if let Some(folder) = folder {
        super::launch::require_isolated_token(folder)?;
        verify_file_auth(program, timeout)?;
        super::launch::prepare_file_auth(folder)?;
    }
    let env = super::launch::environment_for_account(folder).resolve(std::env::vars());
    read_models_with(program, &env, timeout)
}

/// An empty private profile must be signed out, even if the app is signed in.
/// Refuse an unrecognized response rather than opening an unsafe login.
pub fn verify_file_auth(program: &Path, timeout: Duration) -> Result<(), String> {
    use std::sync::{Mutex, OnceLock};
    static VERIFIED: OnceLock<Mutex<BTreeMap<std::path::PathBuf, std::time::SystemTime>>> = OnceLock::new();
    let binary = program.canonicalize().map_err(|e| e.to_string())?;
    let modified = binary.metadata().and_then(|m| m.modified()).map_err(|e| e.to_string())?;
    let mut verified = VERIFIED.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
    if verified.get(&binary) == Some(&modified) { return Ok(()); }
    // The fallback-cache mechanism is internal. Unknown CLI versions must not
    // silently choose a different credential backend, even when the main
    // account happens to be signed out during the empty-profile probe.
    let mut version = Command::new(program);
    crate::platform::no_window(&mut version);
    version.arg("--version").env_clear().envs(super::launch::environment().resolve(std::env::vars())).stdin(Stdio::null());
    let output = crate::agents::auth::run_bounded(version, timeout)?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "1.5.9" {
        return Err("Additional Gemini accounts require the verified Antigravity CLI 1.5.9 file-storage backend. This CLI version has not been validated; the main account is unchanged.".into());
    }
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    super::launch::prepare_file_auth(directory.path())?;
    let env = super::launch::environment_for_account(Some(directory.path())).resolve(std::env::vars());
    match read_models_with(program, &env, timeout) {
        Err(error) if error.contains("Please sign in to view available models") => {
            verified.insert(binary, modified);
            Ok(())
        },
        _ => Err(super::launch::ISOLATION_ERROR.into()),
    }
}

/// Use the CLI's native ACP logout; `/logout` is a local TUI command and
/// cannot safely be sent as a print-mode model prompt.
pub async fn logout_for_account(program: &Path, folder: &Path, timeout: Duration) -> Result<(), String> {
    use crate::transport::{LaunchSpec, Transport};
    use serde_json::json;

    super::launch::require_isolated_token(folder)?;
    let checked_program = program.to_path_buf();
    tokio::task::spawn_blocking(move || verify_file_auth(&checked_program, timeout))
        .await.map_err(|e| e.to_string())??;
    super::launch::prepare_file_auth(folder)?;
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let spec = LaunchSpec {
        program: program.to_path_buf(), args: super::launch::acp_arguments(folder), cwd: directory.path().to_path_buf(),
        env: super::launch::environment_for_account(Some(folder)).with_program_dir(program).resolve(std::env::vars()),
    };
    let (transport, mut events) = Transport::spawn(spec, 0).await.map_err(|e| e.to_string())?;
    // Drain diagnostics without retaining or publishing authentication output.
    let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });
    let outcome = async {
        let initialized = transport.request("initialize", json!({"protocolVersion":1,"clientCapabilities":{}}), timeout).await.map_err(|e| e.to_string())?;
        if initialized.pointer("/agentCapabilities/auth/logout").is_none() {
            return Err("This Antigravity CLI does not support native sign-out; the account has been kept.".into());
        }
        // Resolve the saved login without an interactive authenticate request.
        // An unrecognized/expired login fails here rather than opening a browser.
        transport.request("session/new", json!({"cwd":directory.path(),"mcpServers":[]}), timeout).await.map_err(|e| e.to_string())?;
        transport.request("logout", json!({}), timeout).await.map_err(|e| e.to_string())?;
        if super::launch::token_path(folder).exists() {
            return Err("Antigravity did not clear this account's sign-in; the account has been kept.".into());
        }
        Ok(())
    }.await;
    transport.shutdown(Duration::from_secs(2)).await;
    drain.abort();
    outcome
}

/// `agy models` with an environment already resolved (a session's own).
pub fn read_models_with(program: &Path, env: &BTreeMap<String, String>, timeout: Duration) -> Result<Vec<ProviderModel>, String> {
    let mut child = crate::platform::no_window(&mut Command::new(program))
        .arg("models")
        .current_dir(std::env::temp_dir())
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start agy: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("agy did not list its models within {}s", timeout.as_secs()));
            }
        }
    }
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let models = models_from_list(&stdout);
    if models.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("it listed no models").trim().to_string();
        return Err(format!("agy listed no models ({reason}). {SIGN_IN_HINT}"));
    }
    Ok(models)
}

/// `slug<TAB>Display name` per line, after a "Fetching…" banner.
pub fn models_from_list(text: &str) -> Vec<ProviderModel> {
    text.lines()
        .filter_map(|line| {
            let (id, name) = line.split_once('\t')?;
            let id = id.trim();
            if id.is_empty() || id.contains(' ') {
                return None;
            }
            Some(ProviderModel {
                id: id.to_string(),
                name: name.trim().to_string(),
                description: None,
                needs_credits: false,
                // Antigravity's effort is in the slug (`-high`, `-low`) and in
                // `--effort`; the slug is the one the picker offers.
                efforts: Vec::new(),
                default_effort: None,
                input_image: false,
                is_default: false,
            })
        })
        .collect()
}

/// Signed in when `agy models` answers; otherwise the reason.
pub fn read_status(program: &Path, timeout: Duration) -> ProviderAuthStatus {
    read_status_for_account(program, None, timeout)
}

pub fn read_status_for_account(program: &Path, folder: Option<&Path>, timeout: Duration) -> ProviderAuthStatus {
    read_status_with_identity(program, folder, timeout, || true)
}

pub fn read_status_with_identity(program: &Path, folder: Option<&Path>, timeout: Duration, read_identity: impl Fn() -> bool) -> ProviderAuthStatus {
    match read_models_for_account(program, folder, timeout) {
        Ok(_) if !read_identity() => ProviderAuthStatus { provider: Provider::Gemini, logged_in: Some(true), setup_required: None, method: Some("google".into()), account: None, plan: None, problem: None },
        Ok(_) => {
            let identity = super::quota::read_identity_while_active(program, folder, timeout.min(Duration::from_secs(10)), read_identity);
            let setup = matches!(&identity, Err(problem) if problem == super::quota::SETUP_HINT);
            let identity = identity.ok();
            ProviderAuthStatus { provider: Provider::Gemini, logged_in: Some(true), setup_required: setup.then_some(true), method: Some("google".into()), account: identity.as_ref().and_then(|value| value.0.clone()), plan: identity.and_then(|value| value.1), problem: setup.then(|| super::quota::SETUP_HINT.to_string()) }
        },
        Err(error) => ProviderAuthStatus { provider: Provider::Gemini, logged_in: Some(false), setup_required: None, method: None, account: None, plan: None, problem: Some(error) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the official agy 1.5.9 binary; needs no Google account"]
    fn installed_cli_file_backend_is_signed_out_without_ssh() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from).unwrap();
        let program = std::env::var_os(super::super::launch::PROGRAM_ENV).map(std::path::PathBuf::from)
            .or_else(|| super::super::launch::find_program(&home)).expect("installed agy");
        verify_file_auth(&program, Duration::from_secs(15)).expect("the verified CLI must select file storage without SSH");
    }

    #[test]
    #[ignore = "requires a signed-in local agy installation; sends no model prompt"]
    fn a_new_account_cannot_use_the_main_accounts_credentials() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from).unwrap();
        let program = super::super::launch::find_program(&home).expect("installed agy");
        verify_file_auth(&program, Duration::from_secs(15)).expect("the installed CLI must isolate file-based auth");
        let main = read_status(&program, Duration::from_secs(15));
        assert_eq!(main.logged_in, Some(true), "the default account must be signed in for this check");
        assert!(main.account.is_some(), "the signed-in email must be available from the status line");
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("account");
        crate::agents::accounts::share(Provider::Gemini, &home.join(".gemini"), &folder, &home).unwrap();
        let extra = read_status_for_account(&program, Some(&folder), Duration::from_secs(15));
        assert_eq!(extra.logged_in, Some(false), "a new account must not reuse the main keyring or credential files");
        assert!(extra.problem.unwrap_or_default().contains("isolated sign-in token"));
        assert_eq!(read_status(&program, Duration::from_secs(15)).logged_in, Some(true), "the default account must remain signed in");
    }

    #[tokio::test]
    #[ignore = "requires a signed-in local agy; uses only a dummy token in a temporary profile"]
    async fn native_logout_keeps_an_unrecognized_private_token_instead_of_claiming_success() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from).unwrap();
        let program = super::super::launch::find_program(&home).expect("installed agy");
        let checked_program = program.clone();
        let before = tokio::task::spawn_blocking(move || read_status(&checked_program, Duration::from_secs(15))).await.unwrap();
        assert_eq!(before.logged_in, Some(true));
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".gemini/antigravity-cli")).unwrap();
        std::fs::write(super::super::launch::token_path(directory.path()), "{}").unwrap();
        assert!(logout_for_account(&program, directory.path(), Duration::from_secs(15)).await.is_err());
        assert!(super::super::launch::token_path(directory.path()).exists());
        let after = tokio::task::spawn_blocking(move || read_status(&program, Duration::from_secs(15))).await.unwrap();
        assert_eq!(after.logged_in, Some(true));
        assert_eq!(after.account, before.account, "native logout of a private profile must not switch the main account");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_logout_requires_capability_and_confirmation_that_the_token_is_gone() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        std::fs::create_dir_all(profile.join(".gemini/antigravity-cli")).unwrap();
        let token = super::super::launch::token_path(&profile);
        std::fs::write(&token, "private-token").unwrap();
        let program = directory.path().join("agy");
        for (capability, remove, success) in [(false, false, false), (true, false, false), (true, true, true)] {
            let capabilities = if capability { r#"{"auth":{"logout":{}}}"# } else { "{}" };
            let clear = if remove { "rm -- \"$HOME/.gemini/antigravity-cli/antigravity-oauth-token\"" } else { ":" };
            let script = format!(r#"#!/bin/sh
if [ "$1" = --version ]; then echo 1.5.9; exit 0; fi
if [ "$1" = models ]; then echo 'Please sign in to view available models' >&2; exit 1; fi
while IFS= read -r line; do
 case "$line" in
  *'"method":"initialize"'*) echo '{{"jsonrpc":"2.0","id":1,"result":{{"agentCapabilities":{capabilities}}}}}' ;;
  *'"method":"session/new"'*) echo '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"logout-probe"}}}}' ;;
  *'"method":"logout"'*) {clear}; echo '{{"jsonrpc":"2.0","id":3,"result":{{}}}}' ;;
 esac
done
"#);
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(logout_for_account(&program, &profile, Duration::from_secs(2)).await.is_ok(), success);
            assert_eq!(token.exists(), !success);
        }
    }

    #[test]
    fn the_catalog_reads_as_agy_prints_it() {
        let models = models_from_list("Fetching available models...\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\ngemini-3.1-pro-high\tGemini 3.1 Pro (High)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["gemini-3.8-flash-low", "gemini-3.1-pro-high", "claude-opus-4-6-thinking"]);
        assert_eq!(models[1].name, "Gemini 3.1 Pro (High)");
        assert!(models_from_list("Fetching available models...\n").is_empty());
    }

    #[test]
    fn an_unsigned_profile_never_probes_a_cli_that_might_use_the_shared_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_models_for_account(Path::new("/must-not-run"), Some(dir.path()), Duration::from_secs(1)).unwrap_err();
        assert_eq!(error, super::super::launch::ISOLATION_ERROR);
    }

    #[cfg(unix)]
    #[test]
    fn the_login_preflight_refuses_shared_auth_and_unknown_failures() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("agy");
        let write = |body: &str| {
            std::fs::write(&program, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.5.9; exit 0; fi\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        write("printf 'gemini-test\\tGemini Test\\n'");
        assert!(verify_file_auth(&program, Duration::from_secs(2)).is_err());
        write("echo 'network unavailable' >&2; exit 1");
        assert!(verify_file_auth(&program, Duration::from_secs(2)).is_err());
        let count = dir.path().join("checks");
        write(&format!("echo probe >> '{}'\necho 'Please sign in to view available models' >&2; exit 1", count.display()));
        assert!(verify_file_auth(&program, Duration::from_secs(2)).is_ok());
        assert!(verify_file_auth(&program, Duration::from_secs(2)).is_ok());
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
        let modified = program.metadata().unwrap().modified().unwrap();
        std::fs::File::open(&program).unwrap().set_modified(modified + Duration::from_secs(2)).unwrap();
        assert!(verify_file_auth(&program, Duration::from_secs(2)).is_ok());
        assert_eq!(std::fs::read_to_string(count).unwrap().lines().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn unknown_cli_versions_are_refused_even_when_they_report_signed_out() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("agy");
        std::fs::write(&program, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.5.10; exit 0; fi\necho 'Please sign in to view available models' >&2; exit 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(verify_file_auth(&program, Duration::from_secs(2)).unwrap_err().contains("has not been validated"));
    }

    #[test]
    fn every_platform_the_desktop_runs_on_has_a_release_manifest() {
        // The desktop ships for these platforms; a build here whose name
        // we do not know would silently never find an update.
        assert!(platform().is_some(), "no agy manifest for {}/{}", std::env::consts::OS, std::env::consts::ARCH);
    }

    #[test]
    #[ignore = "asks the public Antigravity release server"]
    fn the_release_manifest_names_a_version() {
        let version = latest_version(Duration::from_secs(30)).expect("a published version");
        assert!(crate::agents::versions::version_in(&version).is_some(), "not a version: {version}");
    }
}
