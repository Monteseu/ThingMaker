//! Finds, and on request installs, what live Android video needs: adb,
//! scrcpy and FFmpeg. Windows uses winget, macOS uses Homebrew.
use super::CommandResult;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use thingmaker_supervisor::DesktopError;
use tokio::process::Command;

fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    }
}

/// Looks for `file` at most `depth` folders below `dir`. winget unpacks
/// portable packages into versioned folders whose PATH entry only reaches
/// processes started after the install, so the app must look there itself.
fn search(dir: &Path, file: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(file);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    entries.into_iter().rev().find_map(|p| search(&p, file, depth - 1))
}

fn winget_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Microsoft/WinGet/Packages"));
    }
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(programs).join("WinGet/Packages"));
    }
    roots
}

/// Find a tool even when launched from Finder rather than a login shell, or
/// right after this app installed it.
pub(super) fn find(name: &str) -> Option<PathBuf> {
    let file = executable(name);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft/WinGet/Links"));
    }
    if let Some(found) = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file()) {
        return Some(found);
    }
    if cfg!(windows) {
        for root in winget_roots() {
            let Ok(packages) = std::fs::read_dir(&root) else {
                continue;
            };
            for package in packages.filter_map(|e| e.ok()) {
                if let Some(found) = search(&package.path(), &file, 3) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The real folder of the installed scrcpy, following winget/Homebrew links:
/// its server jar (and on Windows its bundled adb) lives beside it.
pub(super) fn scrcpy_dir() -> Option<PathBuf> {
    let scrcpy = find("scrcpy")?;
    let real = std::fs::canonicalize(&scrcpy).unwrap_or(scrcpy);
    real.parent().map(Path::to_path_buf)
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Adb,
    Scrcpy,
    Ffmpeg,
}
impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Adb => "Android platform tools (adb)",
            Tool::Scrcpy => "scrcpy",
            Tool::Ffmpeg => "FFmpeg",
        }
    }
    fn present(self) -> bool {
        match self {
            Tool::Adb => super::screen::adb().is_file(),
            Tool::Scrcpy => find("scrcpy").is_some(),
            Tool::Ffmpeg => find("ffmpeg").is_some(),
        }
    }
}

fn missing() -> Vec<Tool> {
    [Tool::Adb, Tool::Scrcpy, Tool::Ffmpeg]
        .into_iter()
        .filter(|tool| !tool.present())
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTools {
    /// Human names of the tools that are not installed.
    missing: Vec<String>,
    /// Whether this app can install them by itself on this computer.
    can_install: bool,
    /// What to run by hand when it cannot.
    manual: Option<String>,
}

fn brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn status() -> ScreenTools {
    let missing = missing();
    let (can_install, manual) = if cfg!(windows) {
        (true, None)
    } else if cfg!(target_os = "macos") {
        match brew() {
            Some(_) => (true, None),
            None => (
                false,
                Some("Install Homebrew from https://brew.sh, then try again".to_string()),
            ),
        }
    } else {
        (false, Some("sudo apt install adb scrcpy ffmpeg".to_string()))
    };
    ScreenTools {
        missing: missing.iter().map(|t| t.label().to_string()).collect(),
        can_install: can_install && !missing.is_empty(),
        manual: (!missing.is_empty()).then_some(manual).flatten(),
    }
}

/// The package-manager commands that install `missing`, de-duplicated.
fn plan(missing: &[Tool]) -> Vec<Vec<String>> {
    let mut steps: Vec<Vec<String>> = Vec::new();
    let mut add = |step: Vec<&str>| {
        let step: Vec<String> = step.into_iter().map(String::from).collect();
        if !steps.contains(&step) {
            steps.push(step);
        }
    };
    for tool in missing {
        if cfg!(windows) {
            // scrcpy's Windows release ships its own adb.exe beside it.
            let id = match tool {
                Tool::Adb | Tool::Scrcpy => "Genymobile.scrcpy",
                Tool::Ffmpeg => "Gyan.FFmpeg",
            };
            add(vec![
                "install",
                "--id",
                id,
                "--exact",
                "--source",
                "winget",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--disable-interactivity",
            ]);
        } else {
            add(match tool {
                Tool::Adb => vec!["install", "--cask", "android-platform-tools"],
                Tool::Scrcpy => vec!["install", "scrcpy"],
                Tool::Ffmpeg => vec!["install", "ffmpeg"],
            });
        }
    }
    steps
}

static INSTALLING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn screen_tools_status() -> ScreenTools {
    status()
}

#[tauri::command]
pub async fn screen_tools_install() -> CommandResult<ScreenTools> {
    let manager = if cfg!(windows) {
        find("winget").unwrap_or_else(|| "winget.exe".into())
    } else if cfg!(target_os = "macos") {
        brew().ok_or_else(|| {
            DesktopError::not_ready("Install Homebrew from https://brew.sh, then try again")
        })?
    } else {
        return Err(DesktopError::unsupported(
            "Install adb, scrcpy and FFmpeg with your package manager",
        ));
    };
    if INSTALLING.swap(true, Ordering::AcqRel) {
        return Err(DesktopError::conflict("The device tools are already installing"));
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            INSTALLING.store(false, Ordering::Release);
        }
    }
    let _release = Release;
    let mut failure = None;
    for step in plan(&missing()) {
        let mut command = Command::new(&manager);
        command
            .args(&step)
            .env("NONINTERACTIVE", "1")
            .env("HOMEBREW_NO_ENV_HINTS", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        match tokio::time::timeout(Duration::from_secs(15 * 60), command.output()).await {
            Err(_) => failure = Some("Installing the device tools timed out".to_string()),
            Ok(Err(_)) => {
                failure = Some(if cfg!(windows) {
                    "winget is not available. Install App Installer from the Microsoft Store, then try again".into()
                } else {
                    "Homebrew could not start".into()
                })
            }
            Ok(Ok(out)) if !out.status.success() => {
                let text = String::from_utf8_lossy(&out.stderr).to_string()
                    + &String::from_utf8_lossy(&out.stdout);
                let tail: Vec<_> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.len() > 2)
                    .collect();
                let tail = tail[tail.len().saturating_sub(3)..].join(" ");
                failure = Some(format!("{} failed: {tail}", step.join(" ")));
            }
            Ok(Ok(_)) => {}
        }
    }
    // Package managers report "already installed" as a failure; what counts
    // is whether every tool can now be found.
    let result = status();
    match failure {
        Some(message) if !result.missing.is_empty() => Err(DesktopError::io(message)),
        _ if !result.missing.is_empty() => Err(DesktopError::io(format!(
            "Installed, but cannot find {} yet. Restart ThingMaker and try again",
            result.missing.join(", ")
        ))),
        _ => Ok(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_each_package_once() {
        let steps = plan(&[Tool::Adb, Tool::Scrcpy, Tool::Ffmpeg]);
        if cfg!(windows) {
            assert_eq!(steps.len(), 2);
            assert!(steps[0].contains(&"Genymobile.scrcpy".to_string()));
            assert!(steps[1].contains(&"Gyan.FFmpeg".to_string()));
        } else {
            assert_eq!(steps.len(), 3);
        }
        assert!(plan(&[]).is_empty());
    }
    #[test]
    fn searches_nested_package_folders() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("pkg/ffmpeg-9/bin");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("ffmpeg.exe"), b"").unwrap();
        assert_eq!(
            search(&dir.path().join("pkg"), "ffmpeg.exe", 3),
            Some(nested.join("ffmpeg.exe"))
        );
        assert_eq!(search(&dir.path().join("pkg"), "ffmpeg.exe", 1), None);
    }
}
