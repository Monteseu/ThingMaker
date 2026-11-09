//! Reading a reviewer's reply: the fenced JSON block its turn ends with.
//!
//! Every provider can write one, which is why Ultra-Review asks for it
//! instead of tools some providers cannot load. Reading is forgiving of form
//! (a missing fence, a line number as text, an unknown severity) and strict
//! about substance; what it cannot use, it says, so the reviewer can be asked
//! once to send it again.

use serde_json::Value;

use super::model::{Outcome, Recommendation, Severity};

/// The last JSON object in a reply: a ```json fence, else any fence, else the
/// last balanced `{…}`.
pub fn json_block(reply: &str) -> Result<Value, String> {
    let mut candidates: Vec<&str> = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|n| n + 1).unwrap_or(0);
        let Some(end) = after[body_start..].find("```") else { break };
        candidates.push(&after[body_start..body_start + end]);
        rest = &after[body_start + end + 3..];
    }
    for candidate in candidates.iter().rev() {
        if let Ok(value) = serde_json::from_str::<Value>(candidate.trim())
            && value.is_object()
        {
            return Ok(value);
        }
    }
    if let Some(value) = last_object(reply) {
        return Ok(value);
    }
    if candidates.is_empty() {
        Err("the reply did not end with a fenced JSON block".into())
    } else {
        Err("the JSON block is not valid JSON".into())
    }
}

/// The last balanced, parseable `{…}` in free text.
fn last_object(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut end = bytes.len();
    while let Some(close) = text[..end].rfind('}') {
        let mut depth = 0i32;
        for (index, &byte) in bytes[..=close].iter().enumerate().rev() {
            match byte {
                b'}' => depth += 1,
                b'{' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(value) = serde_json::from_str::<Value>(&text[index..=close])
                            && value.is_object()
                        {
                            return Some(value);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        end = close;
    }
    None
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(|text| clip(text, 4_000))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

fn line(value: &Value) -> Option<u32> {
    match value.get("line")? {
        Value::Number(number) => number.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0),
        Value::String(text) => text.trim().split(|c: char| !c.is_ascii_digit()).find(|part| !part.is_empty()).and_then(|part| part.parse().ok()),
        _ => None,
    }
}

fn severity(value: &Value) -> Option<Severity> {
    value.get("severity").and_then(Value::as_str).and_then(Severity::parse)
}

fn list<'a>(block: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    block.get(key).and_then(Value::as_array).ok_or_else(|| format!("the JSON block has no \"{key}\" list"))
}

/// A finding as a reviewer reported it in its blind review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub title: String,
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub claim: String,
    pub evidence: Option<String>,
    pub fix: Option<String>,
}

/// At most this many findings per reviewer: the most important first.
pub const MAX_FINDINGS: usize = 8;

pub fn findings(reply: &str) -> Result<Vec<RawFinding>, String> {
    let block = json_block(reply)?;
    let mut found = Vec::new();
    for (index, item) in list(&block, "findings")?.iter().enumerate() {
        let Some(title) = text(item, "title") else {
            return Err(format!("finding {} has no \"title\"", index + 1));
        };
        let claim = text(item, "claim").or_else(|| text(item, "description")).unwrap_or_else(|| title.clone());
        found.push(RawFinding {
            title: clip(&title, 200),
            severity: severity(item).unwrap_or(Severity::Medium),
            file: text(item, "file").map(|file| file.trim_start_matches("./").to_string()),
            line: line(item),
            claim,
            evidence: text(item, "evidence"),
            fix: text(item, "fix"),
        });
    }
    found.truncate(MAX_FINDINGS);
    Ok(found)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    Confirm,
    Challenge,
    Refine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawReply {
    pub finding: String,
    pub stance: Stance,
    pub argument: String,
    pub evidence: Option<String>,
    pub severity: Option<Severity>,
    pub fix: Option<String>,
}

fn finding_id(item: &Value) -> Option<String> {
    let raw = match item.get("finding")? {
        Value::String(text) => text.trim().to_ascii_uppercase(),
        Value::Number(number) => format!("F{number}"),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    Some(if raw.starts_with('F') { raw } else { format!("F{raw}") })
}

/// Cross-examination replies, checked against the findings the reviewer was
/// asked about: every one answered, none of its own.
pub fn replies(reply: &str, asked: &[String]) -> Result<Vec<RawReply>, String> {
    let block = json_block(reply)?;
    let mut answered = Vec::new();
    for (index, item) in list(&block, "replies")?.iter().enumerate() {
        let Some(finding) = finding_id(item) else {
            return Err(format!("reply {} has no \"finding\" id", index + 1));
        };
        if !asked.contains(&finding) {
            continue;
        }
        let stance = match item.get("stance").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s.starts_with("confirm") || s == "agree" => Stance::Confirm,
            Some(s) if s.starts_with("challeng") || s == "disagree" || s == "refute" => Stance::Challenge,
            Some(s) if s.starts_with("refin") => Stance::Refine,
            _ => return Err(format!("the reply on {finding} needs a \"stance\": confirm, refine or challenge")),
        };
        let Some(argument) = text(item, "argument").or_else(|| text(item, "reason")) else {
            return Err(format!("the reply on {finding} has no \"argument\""));
        };
        if answered.iter().any(|existing: &RawReply| existing.finding == finding) {
            continue;
        }
        answered.push(RawReply { finding, stance, argument, evidence: text(item, "evidence"), severity: severity(item), fix: text(item, "fix") });
    }
    let missing: Vec<&String> = asked.iter().filter(|id| !answered.iter().any(|reply| &reply.finding == *id)).collect();
    if !missing.is_empty() {
        return Err(format!("these findings have no reply: {}", missing.iter().map(|id| id.as_str()).collect::<Vec<_>>().join(", ")));
    }
    Ok(answered)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Defend,
    Accept,
    Withdraw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRebuttal {
    pub finding: String,
    pub action: Action,
    pub argument: String,
    pub evidence: Option<String>,
    pub severity: Option<Severity>,
    pub fix: Option<String>,
}

pub fn rebuttals(reply: &str, asked: &[String]) -> Result<Vec<RawRebuttal>, String> {
    let block = json_block(reply)?;
    let mut answered = Vec::new();
    for (index, item) in list(&block, "rebuttals")?.iter().enumerate() {
        let Some(finding) = finding_id(item) else {
            return Err(format!("rebuttal {} has no \"finding\" id", index + 1));
        };
        if !asked.contains(&finding) || answered.iter().any(|existing: &RawRebuttal| existing.finding == finding) {
            continue;
        }
        let action = match item.get("action").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s.starts_with("defend") || s == "keep" => Action::Defend,
            Some(s) if s.starts_with("accept") || s == "refine" => Action::Accept,
            Some(s) if s.starts_with("withdr") || s == "retract" => Action::Withdraw,
            _ => return Err(format!("the rebuttal on {finding} needs an \"action\": defend, accept or withdraw")),
        };
        let argument = text(item, "argument").or_else(|| text(item, "reason")).unwrap_or_default();
        if argument.is_empty() && action != Action::Withdraw {
            return Err(format!("the rebuttal on {finding} has no \"argument\""));
        }
        answered.push(RawRebuttal { finding, action, argument, evidence: text(item, "evidence"), severity: severity(item), fix: text(item, "fix") });
    }
    let missing: Vec<&String> = asked.iter().filter(|id| !answered.iter().any(|rebuttal| &rebuttal.finding == *id)).collect();
    if !missing.is_empty() {
        return Err(format!("these findings have no rebuttal: {}", missing.iter().map(|id| id.as_str()).collect::<Vec<_>>().join(", ")));
    }
    Ok(answered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawItem {
    pub finding: String,
    pub outcome: Outcome,
    pub severity: Option<Severity>,
    pub summary: String,
    pub fix: Option<String>,
    pub case_for: Option<String>,
    pub against: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVerdict {
    pub recommendation: Recommendation,
    pub headline: String,
    pub summary: String,
    pub items: Vec<RawItem>,
}

/// The Chair's verdict. A finding it left out is filled in by the engine from
/// the debate, rather than asking again.
pub fn verdict(reply: &str, known: &[String]) -> Result<RawVerdict, String> {
    let block = json_block(reply)?;
    let verdict = block.get("verdict").unwrap_or(&block);
    let recommendation = verdict
        .get("recommendation")
        .and_then(Value::as_str)
        .and_then(Recommendation::parse)
        .ok_or_else(|| "the verdict needs a \"recommendation\": approve, approve_with_fixes or request_changes".to_string())?;
    let headline = text(verdict, "headline").ok_or_else(|| "the verdict has no \"headline\"".to_string())?;
    let summary = text(verdict, "summary").unwrap_or_default();
    let mut items = Vec::new();
    for item in verdict.get("findings").and_then(Value::as_array).into_iter().flatten() {
        let Some(finding) = finding_id(item).filter(|id| known.contains(id)) else { continue };
        if items.iter().any(|existing: &RawItem| existing.finding == finding) {
            continue;
        }
        let outcome = match item.get("outcome").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s.starts_with("confirm") => Outcome::Confirmed,
            Some(s) if s.starts_with("refut") || s == "rejected" => Outcome::Refuted,
            Some(s) if s.starts_with("split") || s.contains("decide") || s == "disputed" => Outcome::Split,
            Some(s) if s.starts_with("withdr") => Outcome::Withdrawn,
            _ => return Err(format!("the verdict on {finding} needs an \"outcome\": confirmed, refuted, split or withdrawn")),
        };
        items.push(RawItem {
            finding,
            outcome,
            severity: severity(item),
            summary: text(item, "summary").unwrap_or_default(),
            fix: text(item, "fix"),
            case_for: text(item, "for"),
            against: text(item, "against"),
        });
    }
    Ok(RawVerdict { recommendation, headline: clip(&headline, 200), summary, items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_fenced_block_is_the_answer() {
        let reply = "Looked at it.\n```json\n{\"draft\":1}\n```\nOn second thought:\n```json\n{\"findings\":[]}\n```\n";
        assert_eq!(json_block(reply).unwrap(), serde_json::json!({"findings": []}));
    }

    #[test]
    fn a_block_without_a_fence_is_found_too() {
        let reply = "Here it is: {\"findings\":[{\"title\":\"Leak\",\"severity\":\"high\"}]} done";
        assert_eq!(findings(reply).unwrap()[0].severity, Severity::High);
        assert!(json_block("no json at all").unwrap_err().contains("did not end"));
        assert!(json_block("```json\n{oops\n```").unwrap_err().contains("not valid"));
    }

    #[test]
    fn findings_are_read_forgivingly() {
        let reply = r#"```json
{"findings":[{"title":"Timer never stops","severity":"Major","file":"./src/a.ts","line":"L42","claim":"It keeps ticking.","evidence":"a.ts:42"},
{"title":"Unknown severity","severity":"spicy"}]}
```"#;
        let found = findings(reply).unwrap();
        assert_eq!(found[0], RawFinding { title: "Timer never stops".into(), severity: Severity::High, file: Some("src/a.ts".into()), line: Some(42), claim: "It keeps ticking.".into(), evidence: Some("a.ts:42".into()), fix: None });
        assert_eq!(found[1].severity, Severity::Medium);
        assert_eq!(found[1].claim, "Unknown severity");
        assert!(findings("```json\n{\"findings\":[{\"severity\":\"low\"}]}\n```").unwrap_err().contains("no \"title\""));
    }

    #[test]
    fn every_asked_finding_needs_a_reply_and_others_are_ignored() {
        let asked = vec!["F1".to_string(), "F3".to_string()];
        let reply = r#"```json
{"replies":[{"finding":"f1","stance":"Challenge","argument":"Not reachable.","severity":"low"},{"finding":3,"stance":"confirm","argument":"Reproduced."},{"finding":"F2","stance":"confirm","argument":"mine"}]}
```"#;
        let read = replies(reply, &asked).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!((read[0].finding.as_str(), read[0].stance, read[0].severity), ("F1", Stance::Challenge, Some(Severity::Low)));
        assert_eq!(read[1].finding, "F3");
        let short = r#"```json
{"replies":[{"finding":"F1","stance":"confirm","argument":"ok"}]}
```"#;
        assert!(replies(short, &asked).unwrap_err().contains("F3"));
        let vague = r#"```json
{"replies":[{"finding":"F1","stance":"maybe","argument":"hm"},{"finding":"F3","stance":"confirm","argument":"ok"}]}
```"#;
        assert!(replies(vague, &asked).unwrap_err().contains("stance"));
    }

    #[test]
    fn a_withdrawal_needs_no_argument_but_a_defence_does() {
        let asked = vec!["F2".to_string()];
        let withdraw = "```json\n{\"rebuttals\":[{\"finding\":\"F2\",\"action\":\"withdraw\"}]}\n```";
        assert_eq!(rebuttals(withdraw, &asked).unwrap()[0].action, Action::Withdraw);
        let defend = "```json\n{\"rebuttals\":[{\"finding\":\"F2\",\"action\":\"defend\"}]}\n```";
        assert!(rebuttals(defend, &asked).unwrap_err().contains("argument"));
    }

    #[test]
    fn the_verdict_keeps_known_findings_and_reads_split_cases() {
        let known = vec!["F1".to_string(), "F2".to_string()];
        let reply = r#"```json
{"verdict":{"recommendation":"approve with fixes","headline":"Two small fixes","summary":"Good change.",
"findings":[{"finding":"F1","outcome":"confirmed","severity":"medium","summary":"Real.","fix":"Guard it."},
{"finding":"F2","outcome":"split","summary":"Disputed.","for":"Profiled.","against":"Too cheap to matter."},
{"finding":"F9","outcome":"confirmed","summary":"Unknown."}]}}
```"#;
        let read = verdict(reply, &known).unwrap();
        assert_eq!(read.recommendation, Recommendation::ApproveWithFixes);
        assert_eq!(read.items.len(), 2);
        assert_eq!(read.items[1].outcome, Outcome::Split);
        assert_eq!(read.items[1].case_for.as_deref(), Some("Profiled."));
        assert!(verdict("```json\n{\"verdict\":{\"headline\":\"x\"}}\n```", &known).unwrap_err().contains("recommendation"));
    }
}
