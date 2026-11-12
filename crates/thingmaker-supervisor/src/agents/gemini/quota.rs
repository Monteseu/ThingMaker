//! Read agy's documented status-line payload without sending a model prompt.
//! The override applies only to this process; account credentials stay in agy.
//! Payload contract: https://antigravity.google/docs/cli/statusline/
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};

use crate::{
    agents::{
        Provider,
        events::{QuotaSnapshot, QuotaStatus, QuotaWindow},
    },
    security::EnvironmentProfile,
};

/// Start an idle TUI and capture its status JSON in a private temporary directory.
/// No commands are typed into the TUI (including trust or sign-in confirmations).
/// A missing sign-in, older CLI, or unexpected prompt simply yields no reading.
pub fn read_quota(program: &Path, home: &Path, timeout: Duration) -> Result<QuotaSnapshot, String> {
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let output = directory.path().join("status.json");
    let command = if cfg!(windows) {
        "more > \"%THINGMAKER_QUOTA_OUTPUT%\""
    } else {
        "cat > \"$THINGMAKER_QUOTA_OUTPUT\""
    };
    let settings = json!({"statusLine": {"type": "command", "command": command, "enabled": true}});
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
    cmd.cwd(home);
    cmd.env_clear();
    for (key, value) in EnvironmentProfile::trusted_local().resolve(std::env::vars()) {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("THINGMAKER_QUOTA_OUTPUT", &output);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("could not start agy: {e}"))?;
    drop(pair.slave);
    // Drain the terminal so rendering cannot block the status-line callback.
    let drain = std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
        }
    });
    let started = Instant::now();
    let result = loop {
        if let Ok(bytes) = std::fs::read(&output)
            && let Ok(payload) = serde_json::from_slice::<Value>(&bytes)
            && let Ok(snapshot) = quota_from_status(&payload, now_unix_ms())
        {
            break Ok(snapshot);
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
    #[ignore = "requires a signed-in local agy installation"]
    fn live_quota_probe() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let snapshot =
            read_quota(&home.join(".local/bin/agy"), &home, Duration::from_secs(20)).unwrap();
        assert!(!snapshot.windows.is_empty());
    }
}
