//! Read agy's documented status-line payload without sending a model prompt.
//! The override applies only to this process; account credentials stay in agy.
//! Payload contract: https://antigravity.google/docs/cli/statusline/
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};

use crate::{
    agents::{
        Provider,
        events::{QuotaSnapshot, QuotaStatus, QuotaWindow},
    },
};

/// What a terminal is asked for its cursor position (DSR 6). The Windows
/// pseudo console asks it before it passes on anything the program draws,
/// and waits for the answer, so a terminal nobody looks at still has to reply.
/// With agy 1.5.8 an unanswered query hangs the probe outright: agy never
/// draws, and the pseudo console does not close when the probe gives up.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// The reply: the top-left corner, which is where a fresh screen starts.
const CURSOR_REPLY: &[u8] = b"\x1b[1;1R";

/// Text only agy's first-run setup draws (agy 1.5.8): a color scheme picker,
/// then its Terms of Service and data-use consent. The status line does not
/// run until someone finishes it, and finishing it means agreeing to terms,
/// which is the person's to do, never this probe's.
const SETUP_SCREENS: [&[u8]; 2] = [b"Choose your color scheme", b"Terms of Service & Data Use"];

/// What to tell someone whose agy is waiting on its first-run setup.
pub const SETUP_HINT: &str = "Antigravity CLI is waiting for its first-run setup (color scheme and terms of service). Run `agy` once in a terminal and finish it; ThingMaker will not agree to terms for you.";

/// Start an idle TUI in a private temporary directory and capture its status JSON.
/// No commands are typed into the TUI (including trust, terms or sign-in
/// confirmations); the one thing written to it is the terminal's own answer to
/// a cursor query. A missing sign-in, older CLI, or unexpected prompt simply
/// yields no reading; agy's first-run setup yields `SETUP_HINT` at once.
pub fn read_quota(program: &Path, timeout: Duration) -> Result<QuotaSnapshot, String> {
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    // agy asks whether to trust the folder it starts in. It starts in an empty
    // folder of its own, trusted for this run only (`--settings` is never
    // saved), so nobody's real folder is trusted on their behalf.
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).map_err(|e| e.to_string())?;
    let output = workspace.join("status.json");
    // On Windows the status line runs in that folder and writes beside it by
    // a bare name: agy 1.5.8 hands Windows commands to cmd.exe exactly as
    // written, and a quoted path then fails ("The filename, directory name,
    // or volume label syntax is incorrect"), while an unquoted one would
    // break on a profile path with a space in it.
    let command = if cfg!(windows) {
        "more > status.json"
    } else {
        "cat > \"$THINGMAKER_QUOTA_OUTPUT\""
    };
    let settings = json!({
        "statusLine": {"type": "command", "command": command, "enabled": true},
        "trustedWorkspaces": [workspace],
    });
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(["--settings", &settings.to_string()]);
    cmd.cwd(&workspace);
    cmd.env_clear();
    for (key, value) in super::launch::environment().resolve(std::env::vars()) {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("THINGMAKER_QUOTA_OUTPUT", &output);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("could not start agy: {e}"))?;
    drop(pair.slave);
    let in_setup = Arc::new(AtomicBool::new(false));
    // Drain the terminal so rendering cannot block the status-line callback,
    // answering cursor queries and watching for the first-run setup on the way.
    let drain = {
        let in_setup = in_setup.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut screen = Screen::default();
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let seen = screen.read(&buffer[..n]);
                if seen.setup {
                    in_setup.store(true, Ordering::Relaxed);
                }
                for _ in 0..seen.cursor_queries {
                    if writer.write_all(CURSOR_REPLY).and_then(|()| writer.flush()).is_err() {
                        break;
                    }
                }
            }
        })
    };
    let started = Instant::now();
    let result = loop {
        if let Ok(bytes) = std::fs::read(&output)
            && let Ok(payload) = serde_json::from_slice::<Value>(&bytes)
            && let Ok(snapshot) = quota_from_status(&payload, now_unix_ms())
        {
            break Ok(snapshot);
        }
        if in_setup.load(Ordering::Relaxed) {
            break Err(SETUP_HINT.into());
        }
        if started.elapsed() >= timeout {
            break Err(
                "Antigravity did not report quota. Open agy once to check sign-in and setup."
                    .into(),
            );
        }
        match child.try_wait() {
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            _ => break Err("Antigravity exited before reporting quota".into()),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    // Closing the terminal and child releases the drain's reader.
    let _ = drain.join();
    result
}

/// What one read of the terminal showed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Seen {
    cursor_queries: usize,
    setup: bool,
}

/// The terminal's output as it streams in. Keeps the end of what came before,
/// so a query or a setup line split across two reads is still found, and
/// found once.
#[derive(Default)]
struct Screen {
    tail: Vec<u8>,
}

impl Screen {
    fn read(&mut self, chunk: &[u8]) -> Seen {
        let carried = self.tail.len();
        self.tail.extend_from_slice(chunk);
        let text = &self.tail;
        // Only matches that end in this chunk count; the rest were counted
        // when they arrived.
        let ends_here = |at: usize, needle: &[u8]| at + needle.len() > carried;
        let cursor_queries = text
            .windows(CURSOR_QUERY.len())
            .enumerate()
            .filter(|(at, window)| *window == CURSOR_QUERY && ends_here(*at, CURSOR_QUERY))
            .count();
        let setup = SETUP_SCREENS.iter().any(|needle| text.windows(needle.len()).any(|window| window == *needle));
        let keep = SETUP_SCREENS.iter().chain([&CURSOR_QUERY]).map(|needle| needle.len()).max().unwrap_or(1) - 1;
        let drop = self.tail.len().saturating_sub(keep);
        self.tail.drain(..drop);
        Seen { cursor_queries, setup }
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Keep buckets separate: exhausting Claude/GPT must not disable Gemini routing.
pub fn quota_from_status(payload: &Value, now: u64) -> Result<QuotaSnapshot, String> {
    let quotas = payload
        .get("quota")
        .and_then(Value::as_object)
        .ok_or("Antigravity reported no quota")?;
    let mut windows = Vec::new();
    for (kind, bucket) in quotas {
        if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(left) = bucket
            .get("remaining_fraction")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
        else {
            continue;
        };
        windows.push(QuotaWindow {
            kind: kind.clone(),
            used_percent: Some((1.0 - left) * 100.0),
            window_minutes: if kind.ends_with("-weekly") {
                Some(10080)
            } else if kind.ends_with("-five-hour") {
                Some(300)
            } else {
                None
            },
            resets_at: bucket
                .get("reset_time")
                .and_then(Value::as_str)
                .and_then(crate::agents::claude::usage_probe::parse_iso8601)
                .or_else(|| {
                    bucket
                        .get("reset_in_seconds")
                        .and_then(Value::as_u64)
                        .map(|seconds| (now / 1000).saturating_add(seconds))
                }),
        });
    }
    windows.sort_by(|a, b| {
        (!a.kind.starts_with("gemini-"), &a.kind).cmp(&(!b.kind.starts_with("gemini-"), &b.kind))
    });
    if windows.is_empty() {
        return Err("Antigravity reported no percentage quotas".into());
    }
    // A bucket at capacity is a warning, not an account-wide refusal: other
    // model groups may still be available. Actual turn refusals remain authoritative.
    let status = if windows
        .iter()
        .any(|w| w.used_percent.is_some_and(|n| n >= 75.0))
    {
        QuotaStatus::Warning
    } else {
        QuotaStatus::Allowed
    };
    Ok(QuotaSnapshot {
        provider: Provider::Gemini,
        status,
        windows,
        plan: payload
            .get("plan_tier")
            .and_then(Value::as_str)
            .map(str::to_string),
        observed_at_unix_ms: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_captured_agy_1_5_6_buckets_without_blocking_other_models() {
        let snapshot = quota_from_status(&json!({"plan_tier":"Starter", "quota": {
            "gemini-weekly":{"remaining_fraction":0.9949792,"reset_time":"2026-11-19T08:37:20Z"},
            "3p-weekly":{"remaining_fraction":0,"reset_in_seconds":60}
        }}), 1000).unwrap();
        assert_eq!(snapshot.provider, Provider::Gemini);
        assert_eq!(snapshot.status, QuotaStatus::Warning);
        assert_eq!(snapshot.plan.as_deref(), Some("Starter"));
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(
            snapshot
                .windows
                .iter()
                .find(|w| w.kind == "3p-weekly")
                .unwrap()
                .resets_at,
            Some(61)
        );
        let gemini = snapshot
            .windows
            .iter()
            .find(|w| w.kind == "gemini-weekly")
            .unwrap();
        assert_eq!(gemini.window_minutes, Some(10080));
        assert!((gemini.used_percent.unwrap() - 0.50208).abs() < 0.00001);
        assert!(gemini.resets_at.unwrap() > 1_790_000_000);
    }

    #[test]
    fn missing_disabled_and_absolute_credit_balances_are_not_percentages() {
        for payload in [
            json!({}),
            json!({"quota":{}}),
            json!({"quota":{
                "disabled":{"disabled":true,"remaining_fraction":0},
                "credits":{"remaining_amount":20},
                "invalid":{"remaining_fraction":1.1}
            }}),
        ] {
            assert!(quota_from_status(&payload, 0).is_err());
        }
    }

    #[test]
    fn cursor_queries_are_counted_once_even_when_split_across_reads() {
        let mut screen = Screen::default();
        assert_eq!(screen.read(b"\x1b[6n\x1b[?9001h").cursor_queries, 1);
        assert_eq!(screen.read(b"text\x1b[").cursor_queries, 0);
        assert_eq!(screen.read(b"6n").cursor_queries, 1);
        assert_eq!(screen.read(b"n").cursor_queries, 0);
        assert_eq!(screen.read(b"\x1b[6n\x1b[6n").cursor_queries, 2);
        assert_eq!(screen.read(b"").cursor_queries, 0);
    }

    #[test]
    fn agy_first_run_setup_is_recognised_even_when_split_across_reads() {
        let mut screen = Screen::default();
        assert!(!screen.read(b"\x1b[2JWelcome to Antigravity CLI!\r\n").setup);
        assert!(!screen.read(b"Choose your color").setup);
        assert!(screen.read(b" scheme:\x1b[22m").setup);
        assert!(Screen::default().read(b"Terms of Service & Data Use\r\n").setup);
        assert!(!Screen::default().read(b"Gemini 3.1 Pro (High) \xe2\x94\x82 72% left").setup);
    }

    #[test]
    #[ignore = "requires a signed-in local agy installation"]
    fn live_quota_probe() {
        // `THINGMAKER_AGY` names agy where it is not at ~/.local/bin/agy
        // (on Windows it installs under %LOCALAPPDATA%\agy\bin).
        let home = std::path::PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap());
        let program = std::env::var_os(super::super::launch::PROGRAM_ENV).map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/bin/agy"));
        let snapshot = read_quota(&program, Duration::from_secs(30)).unwrap();
        assert!(!snapshot.windows.is_empty());
    }
}
