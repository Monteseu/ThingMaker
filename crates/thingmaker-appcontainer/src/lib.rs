//! Private mode on Windows: a local agent runs in an AppContainer with no
//! network capabilities, so it, and everything it starts, can reach only
//! this computer (see `docs/plans/local-models.md` §5.4).
//!
//! - **The profile.** One AppContainer profile, [`PROFILE_NAME`], made once
//!   and found again by name.
//! - **Loopback.** An AppContainer cannot reach `127.0.0.1` until it is
//!   exempted with `CheckNetIsolation LoopbackExempt`, which needs an
//!   administrator once; [`request_loopback`] asks with Windows' own prompt.
//! - **Files.** The container sees only what it is granted: [`grant`] adds
//!   an inheritable entry for its SID with `icacls`.
//! - **Launching.** ThingMaker's executable, started with [`LAUNCH_ARG`],
//!   runs [`run_inside`]: it starts the agent in the container with its own
//!   stdin, stdout and stderr, in a job object that ends the agent when the
//!   launcher ends, and exits with the agent's code.
//!
//! On other systems every call says Private mode is Windows-only.

/// The AppContainer's name. Stable: grants and the loopback exemption are
/// attached to the SID derived from it.
pub const PROFILE_NAME: &str = "ThingMaker.LocalModels";

/// The argument that starts ThingMaker's executable as the launcher:
/// `<exe> --thingmaker-appcontainer <program> <args…>`.
pub const LAUNCH_ARG: &str = "--thingmaker-appcontainer";

/// Quotes one argument the way `CommandLineToArgvW` reads it back.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '"')) {
        return arg.to_string();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// A whole command line from a program and its arguments.
pub fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(quote(program)).chain(args.iter().map(|arg| quote(arg))).collect::<Vec<_>>().join(" ")
}

/// Whether `CheckNetIsolation LoopbackExempt -s` output lists `sid`.
pub fn listed_in(output: &str, sid: &str) -> bool {
    let sid = sid.to_ascii_lowercase();
    output.to_ascii_lowercase().contains(&sid)
}

#[cfg(windows)]
mod imp;
#[cfg(windows)]
pub use imp::{ensure_profile, grant, loopback_allowed, request_loopback, run_inside};

#[cfg(not(windows))]
mod unavailable {
    const ONLY: &str = "Private mode with an AppContainer is Windows-only";

    pub fn ensure_profile() -> Result<String, String> {
        Err(ONLY.into())
    }

    pub fn grant(_path: &std::path::Path, _sid: &str, _write: bool) -> Result<(), String> {
        Err(ONLY.into())
    }

    pub fn loopback_allowed(_sid: &str) -> bool {
        false
    }

    pub fn request_loopback(_sid: &str) -> Result<bool, String> {
        Err(ONLY.into())
    }

    pub fn run_inside(_program: &str, _args: &[String]) -> Result<u32, String> {
        Err(ONLY.into())
    }
}
#[cfg(not(windows))]
pub use unavailable::{ensure_profile, grant, loopback_allowed, request_loopback, run_inside};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip_through_windows_quoting() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("with space"), "\"with space\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r"C:\Program Files\nodejs\"), r#""C:\Program Files\nodejs\\""#);
        assert_eq!(command_line(r"C:\node.exe", &["a b".into(), "c".into()]), r#"C:\node.exe "a b" c"#);
    }

    #[test]
    fn the_exemption_list_is_read_whatever_its_case() {
        let output = "List Loopback Exempted AppContainers\n\n[1] -----------------------------------------------------------------\n    Name: thingmaker.localmodels\n    SID:  S-1-15-2-1-2-3\n";
        assert!(listed_in(output, "s-1-15-2-1-2-3"));
        assert!(!listed_in(output, "S-1-15-2-9"));
    }
}
