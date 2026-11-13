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
    let env = super::launch::environment().resolve(std::env::vars());
    read_models_with(program, &env, timeout)
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
    match read_models(program, timeout) {
        Ok(_) => ProviderAuthStatus { provider: Provider::Gemini, logged_in: Some(true), method: Some("google".into()), account: None, plan: None, problem: None },
        Err(error) => ProviderAuthStatus { provider: Provider::Gemini, logged_in: Some(false), method: None, account: None, plan: None, problem: Some(error) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_reads_as_agy_prints_it() {
        let models = models_from_list("Fetching available models...\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\ngemini-3.1-pro-high\tGemini 3.1 Pro (High)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["gemini-3.8-flash-low", "gemini-3.1-pro-high", "claude-opus-4-6-thinking"]);
        assert_eq!(models[1].name, "Gemini 3.1 Pro (High)");
        assert!(models_from_list("Fetching available models...\n").is_empty());
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
