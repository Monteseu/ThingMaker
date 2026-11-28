//! Read-only, bounded version probe; it never signs in or runs inference.

use std::{path::Path, process::Stdio, time::Duration};

pub fn parse_version(output: &str) -> Option<String> {
    let version = output.lines().find_map(|line| line.trim().strip_prefix("GitHub Copilot CLI "))?;
    let version = version.trim().trim_end_matches('.');
    let core = version.split('-').next()?;
    (core.split('.').count() == 3 && core.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
        .then(|| version.to_string())
}

pub fn version(program: &Path, timeout: Duration) -> Result<String, String> {
    let profile = crate::security::EnvironmentProfile::trusted_local().with_program_dir(program).with_set("COPILOT_AUTO_UPDATE", "false");
    let mut command = crate::platform::command(program);
    command.arg("--version").current_dir(std::env::temp_dir()).env_clear().envs(profile.resolve(std::env::vars())).stdin(Stdio::null());
    let output = crate::agents::auth::run_bounded(command, timeout.min(Duration::from_secs(10)))?;
    if !output.status.success() { return Err(format!("Copilot --version failed ({})", output.status)); }
    parse_version(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| "Copilot --version printed no recognized version".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_official_version_banner_without_accepting_other_programs() {
        assert_eq!(parse_version("Package extraction took 1ms\nGitHub Copilot CLI 1.0.116.\nRun 'copilot update' to check for updates."), Some("1.0.116".into()));
        assert_eq!(parse_version("GitHub Copilot CLI 1.0.116-0."), Some("1.0.116-0".into()));
        for invalid in ["1.0.116", "codex 1.0.116", "GitHub Copilot CLI nope", "GitHub Copilot CLI 1..2"] {
            assert_eq!(parse_version(invalid), None);
        }
    }
}
