//! Launch contract for `copilot --acp` (docs/spikes/copilot-cli-provider.md).
//! Session ids and history travel over ACP, never `--continue` or a picker.

use std::path::{Path, PathBuf};

use crate::{acp::launch::SessionId, agents::PermissionStance};

pub const PROGRAM_ENV: &str = "THINGMAKER_COPILOT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotLaunchOptions {
    pub root: PathBuf,
    pub reserved: SessionId,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub resume: Option<String>,
    pub permission_mode: PermissionStance,
    /// Copilot's own configuration directory, not a replacement OS home.
    pub account_home: Option<PathBuf>,
    /// Team guidance on the first prompt of each attachment, including resume.
    pub initial_prompt_prefix: Option<String>,
}

impl CopilotLaunchOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(), reserved: SessionId::reserve(), model: None,
            effort: None, resume: None, permission_mode: PermissionStance::ReadOnly,
            account_home: None, initial_prompt_prefix: None,
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        // --trust-folder is prompt-mode-only (and conflicts with --acp).
        // -C also protects against Windows npm shims resetting a UNC cwd to
        // System32. Grant only this workspace's paths/configuration, not all paths.
        let root = self.root.to_string_lossy().into_owned();
        let mut args = vec!["--acp".into(), "--no-auto-update".into(), "--no-ask-user".into(),
            "-C".into(), root.clone(), "--add-dir".into(), root];
        if self.permission_mode == PermissionStance::ReadOnly {
            args.extend(["--mode".into(), "plan".into()]);
        }
        // ACP permission requests remain subject to the actor's workspace stance.
        if let Some(model) = &self.model { args.extend(["--model".into(), model.clone()]); }
        if let Some(effort) = &self.effort { args.extend(["--reasoning-effort".into(), effort.clone()]); }
        args
    }

    /// Keep the process cwd, CLI cwd and ACP session cwd identical. Never
    /// inherit the desktop's cwd for an empty or sensitive workspace.
    pub fn prepare_workspace(&mut self) -> Result<(), crate::DesktopError> {
        let root = &self.root;
        let sensitive = root.as_os_str().is_empty() || root.parent().is_none()
            || ["USERPROFILE", "HOME", "SystemRoot", "WINDIR"].iter().any(|key| {
                std::env::var_os(key).is_some_and(|value| {
                    let folder = PathBuf::from(value);
                    let folder = folder.canonicalize().unwrap_or(folder);
                    let candidate = root.canonicalize().unwrap_or_else(|_| root.clone());
                    candidate == folder || ((*key == "SystemRoot" || *key == "WINDIR") && candidate.starts_with(folder))
                })
            });
        if sensitive {
            let folder = std::env::temp_dir().join("thingmaker-copilot").join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&folder).map_err(|e| crate::DesktopError::io(e.to_string()))?;
            self.root = folder;
        }
        if !self.root.is_absolute() || !self.root.is_dir() {
            return Err(crate::DesktopError::io("Copilot workspace must be an existing absolute directory"));
        }
        // canonicalize() returns \\?\C:\... on Windows. cmd.exe treats that
        // spelling as UNC and silently changes cwd to the Windows directory.
        #[cfg(windows)]
        if let Some(std::path::Component::Prefix(prefix)) = self.root.components().next()
            && let std::path::Prefix::VerbatimDisk(_) = prefix.kind()
        {
            self.root = PathBuf::from(self.root.as_os_str().to_string_lossy().trim_start_matches(r"\\?\"));
        }
        Ok(())
    }

    pub fn environment(&self) -> Vec<(String, String)> {
        let mut env = vec![("COPILOT_AUTO_UPDATE".into(), "false".into())];
        let default = std::env::var_os("COPILOT_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        if let Some(folder) = self.account_home.as_ref().or(default.as_ref()) {
            env.push(("COPILOT_HOME".into(), folder.to_string_lossy().into_owned()));
        }
        env
    }
}

pub fn mode_id(stance: PermissionStance) -> &'static str {
    match stance { PermissionStance::ReadOnly => "plan", PermissionStance::AcceptEdits => "interactive" }
}

/// An explicit override wins, then PATH and the usual desktop/npm prefixes.
pub fn locate(home: &Path, configured: Option<&Path>, override_path: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = configured { return path.is_file().then(|| path.to_path_buf()); }
    if let Some(path) = override_path { return path.is_file().then_some(path); }
    let path = crate::platform::tool_path();
    let mut dirs: Vec<_> = std::env::split_paths(&path).collect();
    dirs.extend([home.join(".local/bin"), home.join("AppData/Roaming/npm")]);
    let names: &[&str] = if cfg!(windows) { &["copilot.exe", "copilot.cmd", "copilot"] } else { &["copilot"] };
    dirs.into_iter().flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_is_acp_and_keeps_permissions_under_the_actor() {
        let mut options = CopilotLaunchOptions::new("/work");
        assert_eq!(options.arguments(), ["--acp", "--no-auto-update", "--no-ask-user", "-C", "/work", "--add-dir", "/work", "--mode", "plan"]);
        options.permission_mode = PermissionStance::AcceptEdits;
        options.model = Some("model-id".into());
        options.effort = Some("high".into());
        options.resume = Some("saved-id".into());
        assert_eq!(options.arguments(), ["--acp", "--no-auto-update", "--no-ask-user", "-C", "/work", "--add-dir", "/work", "--model", "model-id", "--reasoning-effort", "high"]);
        options.account_home = Some(PathBuf::from("/profile"));
        assert_eq!(options.environment(), [("COPILOT_AUTO_UPDATE".into(), "false".into()), ("COPILOT_HOME".into(), "/profile".into())]);
        let launch = crate::agents::AgentLaunch::Copilot(options);
        assert_eq!(launch.session_id(), "saved-id");
        assert!(launch.is_resume());
        assert!(crate::agents::Provider::Copilot.can_orchestrate());
        assert!(crate::agents::Provider::Copilot.worker_team_server());
        assert!(!crate::agents::Provider::Copilot.can_lead_goal());
    }

    #[test]
    fn workspace_is_explicit_and_empty_root_gets_a_private_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let mut options = CopilotLaunchOptions::new(workspace.path());
        options.prepare_workspace().unwrap();
        assert_eq!(options.root, workspace.path());
        #[cfg(windows)]
        {
            let mut canonical = CopilotLaunchOptions::new(workspace.path().canonicalize().unwrap());
            canonical.prepare_workspace().unwrap();
            assert_eq!(canonical.root, workspace.path());
        }
        let args = options.arguments();
        for flag in ["-C", "--add-dir"] {
            let index = args.iter().position(|arg| arg == flag).unwrap();
            assert_eq!(args[index + 1], workspace.path().to_string_lossy());
        }
        assert!(!args.iter().any(|arg| ["--trust-folder", "--allow-all-paths", "--allow-all-tools"].contains(&arg.as_str())));
        let mut fallback = CopilotLaunchOptions::new("");
        fallback.prepare_workspace().unwrap();
        assert!(fallback.root.is_absolute() && fallback.root.is_dir());
        assert!(fallback.root.starts_with(std::env::temp_dir().join("thingmaker-copilot")));
        std::fs::remove_dir(&fallback.root).unwrap();
        let mut invalid = CopilotLaunchOptions::new("relative-workspace");
        assert!(invalid.prepare_workspace().is_err());
    }

    #[test]
    fn explicit_location_wins_and_invalid_override_does_not_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("copilot.cmd");
        std::fs::write(&binary, "").unwrap();
        assert_eq!(locate(dir.path(), Some(&binary), Some(dir.path().join("missing"))), Some(binary));
        assert_eq!(locate(dir.path(), None, Some(dir.path().join("missing"))), None);
    }
}
