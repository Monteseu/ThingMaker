//! Private mode: a local agent, and everything it starts, can reach only
//! this computer.
//!
//! On macOS the program runs under `sandbox-exec` with a profile that refuses
//! every outgoing connection except to `localhost` and Unix sockets (Ollama,
//! and ThingMaker's own team socket). Children inherit the sandbox, so Bash,
//! git, curl and MCP servers are held to it too. The profile is a constant,
//! never a file anyone could edit.
//!
//! There is no fallback: when the sandbox cannot be used, a local agent does
//! not start.

use std::path::{Path, PathBuf};

/// The macOS profile. Measured in Phase 0: curl, git and a grandchild
/// `sh -> curl` are refused; Ollama and a Unix socket connect.
pub const MACOS_PROFILE: &str = "(version 1)\
(allow default)\
(deny network-outbound)\
(allow network-outbound (remote ip \"localhost:*\"))\
(allow network-outbound (remote unix-socket))";

#[cfg(target_os = "macos")]
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The program and leading arguments that run `executable prefix_args…`
/// inside the sandbox.
pub fn wrap(executable: &Path, prefix_args: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    #[cfg(target_os = "macos")]
    {
        let sandbox = PathBuf::from(SANDBOX_EXEC);
        if !sandbox.is_file() {
            return Err("Private mode is not available on this macOS (sandbox-exec is missing), so a local agent cannot start".into());
        }
        let mut args = vec!["-p".to_string(), MACOS_PROFILE.to_string(), executable.to_string_lossy().into_owned()];
        args.extend(prefix_args.iter().cloned());
        Ok((sandbox, args))
    }
    #[cfg(windows)]
    {
        super::appcontainer::wrap(executable, prefix_args)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (executable, prefix_args);
        Err("Private mode is not available on this system, so a local agent cannot start".into())
    }
}

/// What a sandboxed local agent needs before it starts. On macOS nothing:
/// the profile allows file access and refuses the network. On Windows the
/// AppContainer sees only what it is granted: the workspace and the local
/// home to write in, and the folders of the programs it runs to read.
pub fn prepare(root: &Path, home: &Path, programs: &[PathBuf]) -> Result<(), String> {
    #[cfg(windows)]
    {
        super::appcontainer::prepare(root, home, programs)
    }
    #[cfg(not(windows))]
    {
        let _ = (root, home, programs);
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::Command;

    fn run_wrapped(program: &str, args: &[&str]) -> bool {
        let (sandbox, prefix) = wrap(Path::new(program), &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).unwrap();
        Command::new(sandbox).args(prefix).output().map(|output| output.status.success()).unwrap_or(false)
    }

    #[test]
    fn a_sandboxed_program_reaches_this_computer_and_nothing_else() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                drop(stream);
            }
        });
        assert!(run_wrapped("/usr/bin/nc", &["-z", "-G", "3", "127.0.0.1", &port]), "this computer is reachable");
        assert!(!run_wrapped("/usr/bin/nc", &["-z", "-G", "3", "1.1.1.1", "443"]), "the internet is not");
        assert!(!run_wrapped("/bin/sh", &["-c", "/usr/bin/nc -z -G 3 1.1.1.1 443"]), "nor for what the program starts");
    }
}
