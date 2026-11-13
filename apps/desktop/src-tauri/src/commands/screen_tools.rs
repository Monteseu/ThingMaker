//! Finds, and on request installs, what live Android video needs: adb,
//! scrcpy and FFmpeg. Windows uses winget, macOS uses Homebrew. The host
//! reports the exact commands an install runs; the renderer shows them and
//! installs only what the person confirmed.
use super::CommandResult;
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use thingmaker_supervisor::DesktopError;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

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

const SCRCPY_PACKAGE: &str = "Genymobile.scrcpy";
const FFMPEG_PACKAGE: &str = "Gyan.FFmpeg";

/// The winget packages that can hold `file`: those the plan installs (the
/// scrcpy release bundles adb), plus the official platform tools.
fn packages(file: &str) -> &'static [&'static str] {
    match file {
        "adb.exe" => &[SCRCPY_PACKAGE, "Google.PlatformTools"],
        "scrcpy.exe" => &[SCRCPY_PACKAGE],
        "ffmpeg.exe" => &[FFMPEG_PACKAGE, "Gyan.FFmpeg.Essentials"],
        _ => &[],
    }
}

/// Searches only the folders of `ids`: winget names each `<id>_<source>`.
fn scan(roots: &[PathBuf], ids: &[&str], file: &str) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let mut folders: Vec<PathBuf> = std::fs::read_dir(root)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                ids.iter().any(|id| {
                    name.strip_prefix(id)
                        .is_some_and(|rest| rest.starts_with('_'))
                })
            })
            .map(|e| e.path())
            .collect();
        folders.sort();
        folders.into_iter().find_map(|folder| search(&folder, file, 3))
    })
}

/// How long a search that found nothing stands: device polling asks every
/// few seconds, and a tool installed from a terminal still shows up soon.
const MISS_TTL: Duration = Duration::from_secs(30);

type Found = HashMap<String, (Instant, Option<PathBuf>)>;

fn cache() -> &'static Mutex<Found> {
    static CACHE: OnceLock<Mutex<Found>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Package-folder searches, remembered: a hit while the file is still
/// there, a miss for `MISS_TTL`.
fn in_packages(file: &str) -> Option<PathBuf> {
    let ids = packages(file);
    if ids.is_empty() {
        return None;
    }
    // Held through the search, so concurrent callers wait for one search.
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, found)) = cache.get(file) {
        match found {
            Some(path) if path.is_file() => return Some(path.clone()),
            None if at.elapsed() < MISS_TTL => return None,
            _ => {}
        }
    }
    let found = scan(&winget_roots(), ids, file);
    cache.insert(file.to_string(), (Instant::now(), found.clone()));
    found
}

/// Forgets remembered searches, so a fresh install is found at once.
fn forget() {
    cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
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
        return in_packages(&file);
    }
    None
}

/// The real folder of the installed scrcpy, following winget/Homebrew links:
/// its server jar (and on Windows its bundled adb) lives beside it. `dunce`
/// keeps Windows paths free of the `\\?\` prefix, since they go to adb.
pub(super) fn scrcpy_dir() -> Option<PathBuf> {
    let scrcpy = find("scrcpy")?;
    let real = dunce::canonicalize(&scrcpy).unwrap_or(scrcpy);
    real.parent().map(Path::to_path_buf)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Tool {
    Adb,
    Scrcpy,
    Ffmpeg,
}
impl Tool {
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Os {
    Windows,
    Mac,
    Other,
}

fn os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Other
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Manager {
    Winget,
    Homebrew,
}

/// What to do by hand when the app cannot install; the renderer words it.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Manual {
    Homebrew { url: &'static str },
    /// winget comes with Microsoft's App Installer.
    Winget { url: &'static str },
    Command { command: String },
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTools {
    /// The tools that are not installed.
    missing: Vec<Tool>,
    /// Whether this app can install them by itself on this computer.
    can_install: bool,
    /// The package manager an install runs.
    manager: Option<Manager>,
    /// The exact commands an install runs, for the person to confirm.
    plan: Vec<String>,
    /// What to do by hand when it cannot.
    manual: Option<Manual>,
}

fn brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// winget.exe is an app execution alias, a reparse point that `is_file`
/// may not follow, so only its presence is checked.
fn winget() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft/WindowsApps"));
    }
    dirs.into_iter()
        .map(|dir| dir.join("winget.exe"))
        .find(|path| std::fs::symlink_metadata(path).is_ok())
}

/// The package manager on this computer, if the app can use one.
fn manager() -> Option<PathBuf> {
    match os() {
        Os::Windows => winget(),
        Os::Mac => brew(),
        Os::Other => None,
    }
}

#[derive(Debug, PartialEq)]
struct Step {
    program: PathBuf,
    args: Vec<String>,
}
impl Step {
    /// The command as the person reads it before confirming.
    fn display(&self) -> String {
        let program = self.program.display().to_string();
        let program = if program.contains(' ') {
            format!("\"{program}\"")
        } else {
            program
        };
        std::iter::once(program)
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The package-manager commands that install `missing`, de-duplicated.
fn plan(os: Os, program: &Path, missing: &[Tool]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for tool in missing {
        let args: Vec<&str> = match os {
            Os::Windows => {
                // scrcpy's Windows release ships its own adb.exe beside it.
                let id = match tool {
                    Tool::Adb | Tool::Scrcpy => SCRCPY_PACKAGE,
                    Tool::Ffmpeg => FFMPEG_PACKAGE,
                };
                // Both are portable packages: user scope needs no admin.
                vec![
                    "install",
                    "--id",
                    id,
                    "--exact",
                    "--source",
                    "winget",
                    "--scope",
                    "user",
                    "--silent",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ]
            }
            Os::Mac => match tool {
                Tool::Adb => vec!["install", "--cask", "android-platform-tools"],
                Tool::Scrcpy => vec!["install", "scrcpy"],
                Tool::Ffmpeg => vec!["install", "ffmpeg"],
            },
            Os::Other => return Vec::new(),
        };
        let step = Step {
            program: program.to_path_buf(),
            args: args.into_iter().map(String::from).collect(),
        };
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    steps
}

/// What the app can do about `missing` on `os` with the package manager
/// it found there, if any.
fn assess(os: Os, program: Option<&Path>, missing: Vec<Tool>) -> (ScreenTools, Vec<Step>) {
    let manager = match os {
        Os::Windows => Manager::Winget,
        _ => Manager::Homebrew,
    };
    let program = program.filter(|_| os != Os::Other);
    let manual = match os {
        _ if program.is_some() || missing.is_empty() => None,
        Os::Windows => Some(Manual::Winget {
            url: "https://apps.microsoft.com/detail/9NBLGGH4NNS1",
        }),
        Os::Mac => Some(Manual::Homebrew { url: "https://brew.sh" }),
        Os::Other => Some(Manual::Command {
            command: "sudo apt install adb scrcpy ffmpeg".into(),
        }),
    };
    let steps = program
        .map(|program| plan(os, program, &missing))
        .unwrap_or_default();
    let can_install = !steps.is_empty();
    let tools = ScreenTools {
        manager: can_install.then_some(manager),
        plan: steps.iter().map(Step::display).collect(),
        manual,
        can_install,
        missing,
    };
    (tools, steps)
}

fn status() -> (ScreenTools, Vec<Step>) {
    assess(os(), manager().as_deref(), missing())
}

/// The file-system searches block, so they run off the async runtime.
async fn current() -> CommandResult<(ScreenTools, Vec<Step>)> {
    tokio::task::spawn_blocking(status)
        .await
        .map_err(|_| DesktopError::io("Cannot check the device tools"))
}

/// Why an install did not leave every tool in place; the renderer words it.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Problem {
    /// What needs installing differs from what the person confirmed.
    PlanChanged,
    /// Another install is running.
    Busy,
    /// The package manager is gone.
    Unavailable,
    Cancelled,
    TimedOut { command: String },
    CouldNotStart { command: String },
    Failed { command: String, output: String },
    /// Every command finished, yet some tool cannot be found.
    StillMissing,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenToolsInstall {
    tools: ScreenTools,
    problem: Option<Problem>,
}

/// Judges an install by whether the tools can now be found: package
/// managers report "already installed" as a failure.
fn judge(failure: Option<Problem>, tools: &ScreenTools) -> Option<Problem> {
    if tools.missing.is_empty() {
        None
    } else {
        Some(failure.unwrap_or(Problem::StillMissing))
    }
}

/// The last few meaningful lines of a failed command's output.
fn tail(stdout: &[u8], stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).to_string() + &String::from_utf8_lossy(stdout);
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.len() > 2)
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

/// The running install, to refuse a second one and to cancel it.
static RUNNING: Mutex<Option<CancellationToken>> = Mutex::new(None);

#[tauri::command]
pub async fn screen_tools_status() -> CommandResult<ScreenTools> {
    Ok(current().await?.0)
}

/// Runs the install commands, but only those in `plan`: the commands the
/// person saw and confirmed.
#[tauri::command]
pub async fn screen_tools_install(plan: Vec<String>) -> CommandResult<ScreenToolsInstall> {
    let (tools, steps) = current().await?;
    let refuse = |tools, problem| {
        Ok(ScreenToolsInstall {
            tools,
            problem: Some(problem),
        })
    };
    if !tools.missing.is_empty() && !tools.can_install {
        return refuse(tools, Problem::Unavailable);
    }
    if !tools.plan.iter().all(|step| plan.contains(step)) {
        return refuse(tools, Problem::PlanChanged);
    }
    let token = CancellationToken::new();
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.is_some() {
            return refuse(tools, Problem::Busy);
        }
        *running = Some(token.clone());
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    let _release = Release;
    let mut failure = None;
    for step in steps {
        let shown = step.display();
        let mut command = Command::new(&step.program);
        command
            .args(&step.args)
            .env("NONINTERACTIVE", "1")
            .env("HOMEBREW_NO_ENV_HINTS", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let run = tokio::time::timeout(Duration::from_secs(15 * 60), command.output());
        // Dropping the unfinished run kills the package manager.
        let outcome = tokio::select! {
            outcome = run => outcome,
            _ = token.cancelled() => {
                failure = Some(Problem::Cancelled);
                break;
            }
        };
        match outcome {
            Err(_) => failure = Some(Problem::TimedOut { command: shown }),
            Ok(Err(_)) => failure = Some(Problem::CouldNotStart { command: shown }),
            Ok(Ok(out)) if !out.status.success() => {
                failure = Some(Problem::Failed {
                    command: shown,
                    output: tail(&out.stdout, &out.stderr),
                })
            }
            Ok(Ok(_)) => {}
        }
    }
    forget();
    let (tools, _) = current().await?;
    let problem = judge(failure, &tools);
    Ok(ScreenToolsInstall { tools, problem })
}

/// Stops a running install; whatever was already installed stays.
#[tauri::command]
pub fn screen_tools_cancel() {
    if let Some(token) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_each_package_once() {
        let all = [Tool::Adb, Tool::Scrcpy, Tool::Ffmpeg];
        let winget = Path::new("winget.exe");
        let steps = plan(Os::Windows, winget, &all);
        assert_eq!(steps.len(), 2);
        assert!(steps[0].args.contains(&"Genymobile.scrcpy".to_string()));
        assert!(steps[1].args.contains(&"Gyan.FFmpeg".to_string()));
        assert!(steps[0].display().contains("--scope user"));
        assert_eq!(plan(Os::Mac, Path::new("/opt/homebrew/bin/brew"), &all).len(), 3);
        assert!(plan(Os::Windows, winget, &[]).is_empty());
    }
    #[test]
    fn shows_the_exact_commands_it_runs() {
        let (tools, steps) = assess(
            Os::Mac,
            Some(Path::new("/opt/homebrew/bin/brew")),
            vec![Tool::Adb, Tool::Ffmpeg],
        );
        assert!(tools.can_install);
        assert_eq!(tools.manager, Some(Manager::Homebrew));
        assert_eq!(
            tools.plan,
            [
                "/opt/homebrew/bin/brew install --cask android-platform-tools",
                "/opt/homebrew/bin/brew install ffmpeg",
            ]
        );
        assert_eq!(tools.plan, steps.iter().map(Step::display).collect::<Vec<_>>());
        assert_eq!(tools.manual, None);
        let spaced = Step {
            program: PathBuf::from(r"C:\Program Files\winget.exe"),
            args: vec!["install".into()],
        };
        assert_eq!(spaced.display(), r#""C:\Program Files\winget.exe" install"#);
    }
    #[test]
    fn windows_without_winget_explains_instead_of_installing() {
        let (tools, steps) = assess(Os::Windows, None, vec![Tool::Scrcpy]);
        assert!(!tools.can_install);
        assert!(steps.is_empty() && tools.plan.is_empty());
        assert_eq!(tools.manager, None);
        assert!(matches!(tools.manual, Some(Manual::Winget { .. })));
        let json = serde_json::to_value(&tools).unwrap();
        assert_eq!(json["missing"], serde_json::json!(["scrcpy"]));
        assert_eq!(json["manual"]["kind"], "winget");
        let (tools, _) = assess(Os::Windows, Some(Path::new("winget.exe")), vec![Tool::Scrcpy]);
        assert!(tools.can_install);
        assert_eq!(tools.manager, Some(Manager::Winget));
        assert!(tools.plan[0].contains("--accept-package-agreements"));
        let (tools, _) = assess(Os::Windows, None, vec![]);
        assert_eq!(tools.manual, None);
    }
    #[test]
    fn judges_an_install_by_what_can_be_found_after() {
        let (found, _) = assess(Os::Mac, None, vec![]);
        let (lacking, _) = assess(Os::Mac, None, vec![Tool::Ffmpeg]);
        let failed = || Problem::Failed {
            command: "brew install ffmpeg".into(),
            output: "already installed".into(),
        };
        // "Already installed" exits non-zero, yet the tools are there.
        assert_eq!(judge(Some(failed()), &found), None);
        assert_eq!(judge(Some(Problem::Cancelled), &found), None);
        assert_eq!(judge(Some(failed()), &lacking), Some(failed()));
        assert_eq!(judge(None, &lacking), Some(Problem::StillMissing));
        assert_eq!(judge(None, &found), None);
        let json = serde_json::to_value(Problem::TimedOut { command: "x".into() }).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "timedOut", "command": "x" }));
    }
    #[test]
    fn keeps_the_last_lines_of_failed_output() {
        assert_eq!(tail(b"one\ntwo\n", b"three\nfour\n.\n"), "four one two");
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
    #[test]
    fn scans_only_the_planned_package_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for folder in [
            "Other.Tool_Microsoft.Winget.Source_8wekyb3d8bbwe/bin",
            "Gyan.FFmpegX_Microsoft.Winget.Source_8wekyb3d8bbwe/bin",
            "Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-9.0.2-full_build/bin",
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("ffmpeg.exe"), b"").unwrap();
        }
        let found = scan(&[root.clone()], packages("ffmpeg.exe"), "ffmpeg.exe").unwrap();
        assert!(found.starts_with(root.join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe")));
        assert_eq!(scan(&[root], packages("scrcpy.exe"), "scrcpy.exe"), None);
        assert!(packages("winget.exe").is_empty());
    }
}
