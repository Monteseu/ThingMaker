//! Account quota via the experimental SDK RPC; no model session or credentials.
use std::{io::{BufRead, BufReader, Read, Write}, path::{Path, PathBuf}, process::Stdio, sync::{Mutex, OnceLock}, time::{Duration, Instant}};
use serde_json::{Value, json};
use crate::agents::{Provider, auth::ProviderAuthStatus, events::{QuotaSnapshot, QuotaStatus, QuotaWindow}};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

fn account_folder(folder: Option<&Path>) -> Option<PathBuf> {
    folder.map(Path::to_path_buf).or_else(|| std::env::var_os("COPILOT_HOME").map(PathBuf::from).filter(|p| p.is_absolute()))
}

/// Short-lived, account-scoped cache for repeated settings/team refreshes.
/// The lock is never held while starting or waiting for a CLI process.
pub fn read_status(program: &Path, folder: Option<&Path>, timeout: Duration) -> ProviderAuthStatus {
    type Entry = ((PathBuf, Option<PathBuf>), Instant, ProviderAuthStatus);
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let folder = account_folder(folder);
    let key = (program.to_path_buf(), folder.clone());
    let fresh = |at: &Instant, status: &ProviderAuthStatus| at.elapsed() < Duration::from_secs(if status.logged_in.is_some() { 20 } else { 2 });
    if let Some((_, _, status)) = cache.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(k, at, s)| *k == key && fresh(at, s)) {
        return status.clone();
    }
    let status = match probe(program, folder.as_deref(), timeout, || true) {
        Ok(response) => auth_from_response(&response),
        Err(problem) => ProviderAuthStatus::problem(Provider::Copilot, problem),
    };
    let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
    cache.retain(|(k, at, s)| *k != key && fresh(at, s));
    if cache.len() >= 16 { cache.remove(0); }
    cache.push((key, Instant::now(), status.clone()));
    status
}

fn auth_from_response(response: &Value) -> ProviderAuthStatus {
    let mut status = ProviderAuthStatus::problem(Provider::Copilot, "Copilot account.getQuota returned an unrecognized response.");
    if let Some(error) = response.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let forbidden = [error.get("code"), error.get("data").and_then(|d| d.get("status")), error.get("data").and_then(|d| d.get("statusCode"))]
            .iter().flatten().any(|code| **code == json!(403) || **code == json!("403"));
        if forbidden || message.contains("not authenticated") || message.contains("authentication required") || message.split(|c: char| !c.is_ascii_digit()).any(|s| s == "403") {
            status.logged_in = Some(false);
            status.problem = Some("Copilot is not authenticated. Run `copilot login` in a terminal.".into());
        } else {
            // Never forward a raw SDK error: it can contain credential details.
            status.problem = Some("Copilot account.getQuota failed; authentication could not be checked.".into());
        }
    } else if response.pointer("/result/quotaSnapshots").is_some_and(Value::is_object) {
        // Even an empty/exhausted quota is a successfully authenticated account.
        status.logged_in = Some(true);
        status.method = Some("copilot".into());
        status.problem = None;
    }
    // Global getCurrentAuth/getAllUsers may expose secrets. The credential-free
    // accounts.getActive API is session-scoped; do not create a model session.
    status
}

/// Unsupported CLI, missing auth, cancellation and timeout all mean unknown.
pub fn read_quota(program: &Path, folder: Option<&Path>, timeout: Duration, is_active: impl Fn() -> bool) -> Option<QuotaSnapshot> {
    let response = probe(program, folder, timeout, is_active).ok()?;
    if response.get("error").is_some() { return None; }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    quota_from_response(response.get("result")?, now)
}

fn probe(program: &Path, folder: Option<&Path>, timeout: Duration, is_active: impl Fn() -> bool) -> Result<Value, String> {
    let timeout = timeout.min(PROBE_TIMEOUT);
    let deadline = Instant::now() + timeout;
    if !is_active() { return Err("Copilot account probe cancelled.".into()); }
    let mut profile = crate::security::EnvironmentProfile::trusted_local()
        .with_program_dir(program).with_set("COPILOT_AUTO_UPDATE", "false");
    if let Some(folder) = account_folder(folder) { profile = profile.with_set("COPILOT_HOME", folder.to_string_lossy()); }
    let mut child = crate::platform::command(program)
        .args(["--server", "--stdio", "--no-auto-update"])
        .current_dir(std::env::temp_dir()).env_clear().envs(profile.resolve(std::env::vars()))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("Could not start Copilot account probe: {e}"))?;
    let result = (|| {
        let mut input = child.stdin.take().ok_or("Copilot account probe has no stdin.")?;
        let output = child.stdout.take().ok_or("Copilot account probe has no stdout.")?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Some(value) = read_frame(&mut reader) {
                if value.get("id").and_then(Value::as_u64) == Some(1) && value.get("method").is_none() {
                    let _ = tx.send(value);
                    break;
                }
            }
        });
        let body = json!({"jsonrpc":"2.0", "id":1, "method":"account.getQuota", "params":{}}).to_string();
        write!(input, "Content-Length: {}\r\n\r\n{}", body.len(), body).map_err(|e| format!("Could not send Copilot account probe: {e}"))?;
        input.flush().map_err(|e| format!("Could not flush Copilot account probe: {e}"))?;
        loop {
            if !is_active() { return Err("Copilot account probe cancelled.".into()); }
            if Instant::now() >= deadline { return Err(format!("Copilot account probe did not answer within {}s.", timeout.as_secs())); }
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100))) {
                Ok(value) => return Ok(value),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("Copilot account probe closed without a valid response.".into()),
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

// SDK stdio uses Content-Length, unlike newline-delimited ACP.
fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let count = (&mut *reader).take(8193).read_line(&mut line).ok()?;
        header_bytes += count;
        if count == 0 || header_bytes > 8192 { return None; }
        if line == "\r\n" || line == "\n" { break; }
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case("Content-Length") { length = Some(value.trim().parse::<usize>().ok()?); }
    }
    let length = length.filter(|length| *length <= 1024 * 1024)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

pub fn quota_from_response(response: &Value, observed_at_unix_ms: u64) -> Option<QuotaSnapshot> {
    let buckets = response.get("quotaSnapshots")?.as_object()?;
    let mut buckets: Vec<_> = buckets.iter().collect();
    // Known entitlements first, future SDK buckets in alphabetical order.
    buckets.sort_by_key(|(kind, _)| (match kind.as_str() {
        "premium_interactions" => 0, "chat" => 1, "completions" => 2, _ => 3,
    }, kind.as_str()));
    let mut windows = Vec::new();
    let mut all_exhausted = true;
    let mut warning = false;
    for (kind, bucket) in buckets {
        let unlimited = bucket.get("isUnlimitedEntitlement").and_then(Value::as_bool) == Some(true);
        let applicable = unlimited || bucket.get("hasQuota").and_then(Value::as_bool) == Some(true)
            || bucket.get("entitlementRequests").and_then(Value::as_f64).is_some_and(|n| n > 0.0);
        if !applicable { continue; }
        let used_percent = if unlimited { None } else {
            Some(100.0 - bucket.get("remainingPercentage")?.as_f64().filter(|n| n.is_finite() && (0.0..=100.0).contains(n))?)
        };
        let exhausted = used_percent.is_some_and(|n| n >= 100.0);
        // Spent entitlement can still permit work through paid overage.
        let may_continue = bucket.get("usageAllowedWithExhaustedQuota").and_then(Value::as_bool) == Some(true)
            || bucket.get("overageAllowedWithExhaustedQuota").and_then(Value::as_bool) == Some(true);
        all_exhausted &= exhausted && !may_continue;
        warning |= used_percent.is_some_and(|n| n >= 90.0);
        windows.push(QuotaWindow {
            // Explicit unlimited marker without inventing a percentage.
            kind: if unlimited { format!("{kind}_unlimited") } else { kind.clone() },
            used_percent, window_minutes: None,
            resets_at: bucket.get("resetDate").and_then(Value::as_str).and_then(crate::agents::claude::usage_probe::parse_iso8601),
        });
    }
    if windows.is_empty() { return None; }
    let status = if all_exhausted { QuotaStatus::Rejected }
        else if warning { QuotaStatus::Warning }
        else { QuotaStatus::Allowed };
    Some(QuotaSnapshot {
        provider: Provider::Copilot, status, windows,
        // The RPC does not report a subscription plan name.
        plan: None, observed_at_unix_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_quota_is_ready_even_without_entitlement() {
        for result in [response(24.0), response(0.0), json!({"quotaSnapshots":{}})] {
            let status = auth_from_response(&json!({"id":1,"result":result}));
            assert_eq!(status.provider, Provider::Copilot);
            assert_eq!(status.logged_in, Some(true));
            assert_eq!(status.problem, None);
            assert_eq!(status.account, None);
            assert_eq!(status.plan, None);
            assert_eq!(serde_json::to_value(status).unwrap()["loggedIn"], true);
        }
    }

    #[test]
    fn auth_rejections_are_signed_out_but_other_errors_stay_unknown() {
        for error in [json!({"code":-32603,"message":"Not authenticated. Please authenticate first."}), json!({"code":403}), json!({"code":-32603,"message":"HTTP 403 Forbidden"}), json!({"data":{"statusCode":403}})] {
            assert_eq!(auth_from_response(&json!({"error":error})).logged_in, Some(false));
        }
        for value in [json!({}), json!({"result":{"quotaSnapshots":null}}), json!({"error":{"code":-32601,"message":"Method not found"}}), json!({"error":{"message":"private credential details"},"result":{"quotaSnapshots":{}}})] {
            let status = auth_from_response(&value);
            assert_eq!(status.logged_in, None);
            assert!(status.problem.is_some());
            assert!(!status.problem.unwrap().contains("private credential"));
        }
    }

    #[test]
    fn missing_cli_and_cancelled_probe_are_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("missing-copilot");
        let status = read_status(&program, Some(dir.path()), Duration::from_millis(50));
        assert_eq!(status.logged_in, None);
        assert!(status.problem.unwrap().contains("Could not start Copilot"));
        assert_eq!(read_quota(&program, None, Duration::ZERO, || false), None);
    }

    fn response(remaining: f64) -> Value {
        json!({"quotaSnapshots":{"premium_interactions":{"isUnlimitedEntitlement":false,"remainingPercentage":remaining,"usedRequests":76,"entitlementRequests":100,"resetDate":"2026-12-01T00:00:00Z"}}})
    }
    #[test]
    fn maps_percentage_reset_and_thresholds() {
        let snapshot = quota_from_response(&response(24.0), 42).unwrap();
        assert_eq!(snapshot.provider, Provider::Copilot);
        assert_eq!(snapshot.windows[0].used_percent, Some(76.0));
        assert_eq!(snapshot.windows[0].resets_at, Some(1796083200));
        assert_eq!(snapshot.observed_at_unix_ms, 42);
        assert_eq!(snapshot.plan, None);
        assert_eq!(snapshot.status, QuotaStatus::Allowed);
        assert_eq!(quota_from_response(&response(10.0), 0).unwrap().status, QuotaStatus::Warning);
        assert_eq!(quota_from_response(&response(0.0), 0).unwrap().status, QuotaStatus::Rejected);
    }
    #[test]
    fn unlimited_and_unknown_never_invent_percentages() {
        let unlimited = json!({"quotaSnapshots":{"premium_interactions":{"isUnlimitedEntitlement":true,"remainingPercentage":0}}});
        let snapshot = quota_from_response(&unlimited, 0).unwrap();
        assert_eq!(snapshot.status, QuotaStatus::Allowed);
        assert_eq!(snapshot.windows[0].used_percent, None);
        assert!(snapshot.windows[0].kind.ends_with("_unlimited"));
        for value in [json!({}), json!({"quotaSnapshots":{}}), json!({"quotaSnapshots":{"premium_interactions":{"isUnlimitedEntitlement":false}}}), response(-1.0), response(101.0)] {
            assert_eq!(quota_from_response(&value, 0), None);
        }
    }
    #[test]
    fn overage_does_not_reject_a_usable_account() {
        for flag in ["usageAllowedWithExhaustedQuota", "overageAllowedWithExhaustedQuota"] {
            let mut value = response(0.0);
            value["quotaSnapshots"]["premium_interactions"][flag] = json!(true);
            assert_eq!(quota_from_response(&value, 0).unwrap().status, QuotaStatus::Warning);
        }
    }
    #[test]
    fn real_free_account_omits_unavailable_premium_entitlement() {
        let value = json!({"quotaSnapshots":{
            "chat":{"isUnlimitedEntitlement":false,"entitlementRequests":200,"usedRequests":0,"usageAllowedWithExhaustedQuota":false,"overage":0,"overageAllowedWithExhaustedQuota":false,"remainingPercentage":99.9,"resetDate":"2026-11-27T20:58:58.294Z","hasQuota":true,"tokenBasedBilling":true},
            "completions":{"isUnlimitedEntitlement":false,"entitlementRequests":2000,"usedRequests":0,"usageAllowedWithExhaustedQuota":false,"overage":0,"overageAllowedWithExhaustedQuota":false,"remainingPercentage":100,"resetDate":"2026-11-27T20:58:58.294Z","hasQuota":true,"tokenBasedBilling":true},
            "premium_interactions":{"isUnlimitedEntitlement":false,"entitlementRequests":0,"usedRequests":0,"usageAllowedWithExhaustedQuota":false,"overage":0,"overageAllowedWithExhaustedQuota":false,"remainingPercentage":0,"resetDate":"2026-11-27T20:58:58.294Z","hasQuota":false,"tokenBasedBilling":true}
        }});
        let snapshot = quota_from_response(&value, 42).unwrap();
        assert_eq!(snapshot.status, QuotaStatus::Allowed);
        assert_eq!(snapshot.windows.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["chat", "completions"]);
        assert!((snapshot.windows[0].used_percent.unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(snapshot.windows[1].used_percent, Some(0.0));
        for window in &snapshot.windows { assert_eq!(window.resets_at, Some(1795813138)); }
        let mut unavailable = value.clone();
        unavailable["quotaSnapshots"].as_object_mut().unwrap().retain(|kind, _| kind == "premium_interactions");
        assert_eq!(quota_from_response(&unavailable, 0), None);
    }
    #[test]
    fn rejection_requires_every_applicable_bucket_to_be_blocked() {
        let mut value = response(0.0);
        value["quotaSnapshots"]["chat"] = json!({"hasQuota":true,"remainingPercentage":50});
        assert_eq!(quota_from_response(&value, 0).unwrap().status, QuotaStatus::Warning);
        value["quotaSnapshots"]["chat"]["remainingPercentage"] = json!(0);
        assert_eq!(quota_from_response(&value, 0).unwrap().status, QuotaStatus::Rejected);
        for flag in ["usageAllowedWithExhaustedQuota", "overageAllowedWithExhaustedQuota"] {
            value["quotaSnapshots"]["chat"][flag] = json!(true);
            assert_eq!(quota_from_response(&value, 0).unwrap().status, QuotaStatus::Warning);
            value["quotaSnapshots"]["chat"][flag] = json!(false);
        }
        value["quotaSnapshots"]["chat"] = json!({"isUnlimitedEntitlement":true,"hasQuota":false});
        let snapshot = quota_from_response(&value, 0).unwrap();
        assert_eq!(snapshot.status, QuotaStatus::Warning);
        assert_eq!(snapshot.windows[1].kind, "chat_unlimited");
        assert_eq!(snapshot.windows[1].used_percent, None);
        value["quotaSnapshots"]["future"] = json!({"hasQuota":true,"remainingPercentage":80});
        assert_eq!(quota_from_response(&value, 0).unwrap().windows[2].kind, "future");
    }
    #[test]
    fn sdk_framing_is_bounded_and_rejects_truncated_data() {
        let body = json!({"id":1,"result":response(24.0)}).to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        assert_eq!(read_frame(&mut std::io::Cursor::new(frame)), serde_json::from_str(&body).ok());
        for frame in ["Content-Length: 1048577\r\n\r\n", "Content-Length: 3\r\n\r\n{", "bad\r\n\r\n"] {
            assert!(read_frame(&mut std::io::Cursor::new(frame)).is_none());
        }
    }
}
