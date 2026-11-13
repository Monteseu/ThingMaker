//! Native subprocess construction shared by desktop helpers.
use std::{ffi::OsStr, process::Command};

/// PATH with the folders developer tools are usually installed in added at
/// the end when they exist and are missing. An app opened from the Dock gets
/// the bare system PATH, so git could not find the helpers a repository
/// configures (git-lfs from Homebrew, for one).
pub fn tool_path() -> std::ffi::OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut parts: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let candidates = [Some(std::path::PathBuf::from("/opt/homebrew/bin")), Some(std::path::PathBuf::from("/usr/local/bin")), home.as_ref().map(|home| home.join(".local/bin"))];
    for dir in candidates.into_iter().flatten() {
        if dir.is_dir() && !parts.contains(&dir) {
            parts.push(dir);
        }
    }
    std::env::join_paths(parts).unwrap_or(current)
}

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    #[cfg(windows)]
    let resolved = {
        let path = std::path::Path::new(program);
        if path.components().count() == 1 && path.extension().is_none() {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .flat_map(|dir| ["exe", "cmd", "bat"].map(|extension| dir.join(path).with_extension(extension)))
                .find(|path| path.is_file())
        } else { None }
    };
    #[cfg(windows)]
    let mut command = Command::new(resolved.as_ref().map(|path| path.as_os_str()).unwrap_or(program));
    #[cfg(not(windows))]
    let command = Command::new(program);
    #[cfg(windows)]
    no_window(&mut command);
    command
}

/// Keeps a console program from flashing its own empty console window on
/// Windows: the app is a GUI process, so every child would get one.
pub fn no_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
}

#[cfg(all(test, unix))]
mod path_tests {
    #[test]
    fn the_tool_path_keeps_what_is_there_and_adds_homebrew_when_it_exists() {
        let path = super::tool_path();
        let parts: Vec<_> = std::env::split_paths(&path).collect();
        for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            assert!(parts.contains(&dir));
        }
        let homebrew = std::path::PathBuf::from("/opt/homebrew/bin");
        assert_eq!(parts.contains(&homebrew), homebrew.is_dir() || std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|dir| dir == homebrew));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn batch_wrapper_preserves_a_quoted_argument_in_a_path_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock npm wrapper.cmd");
        std::fs::write(&script, "@echo off\r\necho %~1\r\n").unwrap();
        let output = command(&script).arg("a value with spaces").output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "a value with spaces");
    }
}
