//! What each reviewer is asked, phase by phase. Every prompt stands on its
//! own (the change, the rules, the findings and their threads), so a reviewer
//! that lost its session, or a review resumed after a restart, needs nothing
//! from earlier turns.

use super::model::{Entry, EntryKind, Finding, Review, Reviewer, TargetKind, USER};

/// How much of one thread entry a prompt carries.
const ENTRY_LIMIT: usize = 1_500;

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// Who the reviewer is, what the change is, and the house rules.
fn header(review: &Review, reviewer: &Reviewer) -> String {
    let target = &review.target;
    let what = match target.kind {
        TargetKind::Pr => format!("pull request #{} \"{}\" ({} into {})", target.number.unwrap_or_default(), target.title, target.head_ref, target.base_ref),
        TargetKind::Branch => format!("the branch {} against {} (\"{}\"), uncommitted changes included", target.head_ref, target.base_ref, target.title),
    };
    let mut text = format!(
        "You are Reviewer {id} on an Ultra-Review: a panel of AI reviewers from different providers reviews one change on their own, then examines each other's findings until only what holds up is left. \
The other reviewers are known only by letters. You are Reviewer {id}; do not try to guess who the others are, and weigh what they say by its evidence alone.\n\n\
The change: {what}. {files} files, +{add} −{del}.\n\
You are in a checkout of the change: {root}\n\
See the whole change with `git diff {base}` (and `git diff --stat {base}` for an overview). Read the surrounding code too, not only the diff.\n\n\
Rules:\n",
        id = reviewer.id,
        files = target.files,
        add = target.additions,
        del = target.deletions,
        root = review.root,
        base = short(&target.base_sha),
    );
    if review.config.runs_tests() {
        text.push_str("- You may run the project's tests and builds to back a claim. Do not commit, push or keep any change: a test you write to prove a point goes in a temporary file you delete afterwards.\n");
    } else {
        text.push_str("- Read the code; do not run tests or builds this time. Do not modify, commit or push anything.\n");
    }
    if review.config.require_evidence {
        text.push_str("- Every claim and every challenge needs evidence: a file and line, a test you ran with its result, or a concrete input that breaks it. Opinions without evidence carry no weight.\n");
    }
    text.push_str("- Report real problems worth the author's time: bugs, risks, missing tests, design problems. Not style preferences, unless they cause harm.\n");
    text.push_str(&format!("\nYour lens: {}.", reviewer.lens.brief()));
    if let Some(focus) = reviewer.focus.as_deref().map(str::trim).filter(|focus| !focus.is_empty()) {
        text.push_str(&format!(" The user adds: {focus}"));
    }
    text.push_str("\n\n");
    text
}

fn who(by: &str) -> String {
    if by == USER { "The user (the change's owner)".into() } else { format!("Reviewer {by}") }
}

fn entry_line(entry: &Entry) -> String {
    let what = match entry.kind {
        EntryKind::Claim => "claim",
        EntryKind::Confirm => "confirms",
        EntryKind::Challenge => "challenges",
        EntryKind::Refine => "refines",
        EntryKind::Defend => "defends it",
        EntryKind::Accept => "accepts the refinement",
        EntryKind::Withdraw => "withdraws it",
        EntryKind::Note => "note",
    };
    let mut line = format!("- {} ({what}): {}", who(&entry.by), clip(&entry.text, ENTRY_LIMIT));
    if let Some(evidence) = &entry.evidence {
        line.push_str(&format!("\n  Evidence: {}", clip(evidence, ENTRY_LIMIT)));
    }
    if let Some(severity) = entry.severity.filter(|_| entry.kind != EntryKind::Claim) {
        line.push_str(&format!("\n  Severity they would give: {}", severity.as_str()));
    }
    if let Some(fix) = entry.fix.as_ref().filter(|_| entry.kind != EntryKind::Claim) {
        line.push_str(&format!("\n  Fix they suggest: {}", clip(fix, ENTRY_LIMIT)));
    }
    line
}

/// A finding with its whole thread, as every later prompt shows it.
pub fn render_finding(finding: &Finding) -> String {
    let mut text = format!("### {} · {}\nRaised by Reviewer {} · severity {}", finding.id, finding.title, finding.author, finding.severity.as_str());
    if let Some(location) = finding.location() {
        text.push_str(&format!(" · {location}"));
    }
    text.push('\n');
    if let Some(claim) = finding.thread.first() {
        text.push_str(&format!("Claim: {}\n", clip(&claim.text, ENTRY_LIMIT * 2)));
        if let Some(evidence) = &claim.evidence {
            text.push_str(&format!("Evidence: {}\n", clip(evidence, ENTRY_LIMIT * 2)));
        }
        if let Some(fix) = &claim.fix {
            text.push_str(&format!("Suggested fix: {}\n", clip(fix, ENTRY_LIMIT)));
        }
    }
    let rest: Vec<String> = finding.thread.iter().skip(1).map(entry_line).collect();
    if !rest.is_empty() {
        text.push_str("Discussion so far:\n");
        text.push_str(&rest.join("\n"));
        text.push('\n');
    }
    text
}

fn ending(shape: &str) -> String {
    format!("\nEnd your reply with exactly one fenced JSON block in this shape, and nothing after it:\n```json\n{shape}\n```\n")
}

pub fn blind(review: &Review, reviewer: &Reviewer) -> String {
    let mut text = header(review, reviewer);
    text.push_str(&format!(
        "Blind review. Review the change on your own, through your lens. Report at most {} findings, most important first. \
If nothing is worth reporting, say so and send an empty list: an empty review is a fine result.\n",
        super::parse::MAX_FINDINGS
    ));
    text.push_str(&ending(
        r#"{"findings":[{"title":"the problem in one line","severity":"low|medium|high|critical","file":"path/from/the/repository/root","line":123,"claim":"what is wrong and why it matters","evidence":"how you know: file:line, a test you ran and its output, or an input that breaks it","fix":"the change you suggest (optional)"}]}"#,
    ));
    text
}

pub fn cross(review: &Review, reviewer: &Reviewer, asked: &[String], round: u8) -> String {
    let mut text = header(review, reviewer);
    text.push_str(&format!(
        "Cross-examination, round {}. Below are findings from the other reviewers{}. Check each one against the code yourself, then take a stance:\n\
- confirm: it is real and its severity is right;\n\
- refine: it is real, but something is off (the severity, the scope, the fix): say what;\n\
- challenge: it is wrong, or not worth fixing: show why.\n\
Judge the evidence, not the author, and do not agree just because others did. Answer every finding listed.\n\n",
        round,
        if round > 1 { ", with the discussion so far; answer what changed since your last word" } else { "" }
    ));
    for finding in review.findings.iter().filter(|finding| asked.contains(&finding.id)) {
        text.push_str(&render_finding(finding));
        text.push('\n');
    }
    text.push_str(&ending(
        r#"{"replies":[{"finding":"F1","stance":"confirm|refine|challenge","argument":"your reasoning","evidence":"what you checked or ran","severity":"the severity you think is right (optional)","fix":"a better fix (optional)"}]}"#,
    ));
    text
}

pub fn rebuttal(review: &Review, reviewer: &Reviewer, asked: &[String], round: u8) -> String {
    let mut text = header(review, reviewer);
    text.push_str(&format!(
        "Rebuttal, round {}. Other reviewers answered findings you raised. For each one below, decide:\n\
- defend: it stands as you put it; answer the challenges with evidence;\n\
- accept: take the refinement, and give the severity or fix you now agree with;\n\
- withdraw: the challenges are right.\n\
Withdrawing a finding that does not hold up is a good outcome, not a loss.\n\n",
        round
    ));
    for finding in review.findings.iter().filter(|finding| asked.contains(&finding.id)) {
        text.push_str(&render_finding(finding));
        text.push('\n');
    }
    text.push_str(&ending(
        r#"{"rebuttals":[{"finding":"F1","action":"defend|accept|withdraw","argument":"your answer","evidence":"what backs it (optional)","severity":"the severity you now agree with (optional)","fix":"(optional)"}]}"#,
    ));
    text
}

pub fn verdict(review: &Review, chair: &Reviewer) -> String {
    let mut text = header(review, chair);
    text.push_str(
        "You are also the Chair. The debate is over: write the verdict.\n\
For every finding, weigh the evidence in its thread: not the number of votes, and not who you think wrote it. \
Findings you raised yourself get the same scrutiny as any other. Decide each one:\n\
- confirmed: real; give its severity and the fix;\n\
- refuted: the challenges hold;\n\
- split: a real disagreement the evidence cannot settle; give the best case for and against, so the user can decide;\n\
- withdrawn: its author withdrew it.\n\
Where the user wrote a note, take it into account. Then recommend approve, approve_with_fixes or request_changes, with a one-line headline and a short summary for the change's author.\n\n",
    );
    if review.findings.is_empty() {
        text.push_str("No reviewer reported a finding.\n");
    }
    for finding in &review.findings {
        text.push_str(&render_finding(finding));
        text.push('\n');
    }
    text.push_str(&ending(
        r#"{"verdict":{"recommendation":"approve|approve_with_fixes|request_changes","headline":"one line","summary":"a few sentences","findings":[{"finding":"F1","outcome":"confirmed|refuted|split|withdrawn","severity":"low|medium|high|critical","summary":"what was decided and why","fix":"(confirmed) the fix","for":"(split) the best case it is real","against":"(split) the best case it is not"}]}}"#,
    ));
    text
}

/// Asked once when a reply could not be read.
pub fn correction(error: &str) -> String {
    format!("Your last reply's JSON block could not be used: {error}. Send the block again, corrected, in the same shape. Reply with only the fenced JSON block.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ultrareview::{
        debate::{add_findings, apply_replies},
        parse::{RawFinding, RawReply, Stance},
    };

    fn review() -> Review {
        let mut review = crate::ultrareview::debate::tests_support::review(3);
        review.target.head_sha = "0123456789abcdef".into();
        review.target.base_sha = "fedcba9876543210".into();
        review
    }

    #[test]
    fn the_header_names_the_reviewer_by_letter_and_the_base_to_diff_against() {
        let review = review();
        let text = blind(&review, &review.reviewers[1]);
        assert!(text.contains("You are Reviewer B"));
        assert!(text.contains("git diff fedcba9876"));
        assert!(text.contains("\"findings\""));
        assert!(text.contains("may run the project's tests"));
        assert!(!text.contains("claude"), "no provider names: {text}");
    }

    #[test]
    fn quick_mode_reads_the_code_only() {
        let mut review = review();
        review.config.quick = true;
        assert!(blind(&review, &review.reviewers[0]).contains("do not run tests or builds"));
    }

    #[test]
    fn cross_examination_shows_the_asked_findings_with_their_threads() {
        let mut review = review();
        add_findings(&mut review, "A", vec![RawFinding { title: "Leak".into(), severity: crate::ultrareview::model::Severity::High, file: Some("a.rs".into()), line: Some(9), claim: "It leaks.".into(), evidence: Some("a.rs:9".into()), fix: None }], 1);
        add_findings(&mut review, "C", vec![RawFinding { title: "Other".into(), severity: crate::ultrareview::model::Severity::Low, file: None, line: None, claim: "x".into(), evidence: None, fix: None }], 1);
        apply_replies(&mut review, "C", vec![RawReply { finding: "F1".into(), stance: Stance::Challenge, argument: "Freed in drop.".into(), evidence: None, severity: Some(crate::ultrareview::model::Severity::Low), fix: None }], 1, 2);
        let text = cross(&review, &review.reviewers[1], &["F1".to_string()], 2);
        assert!(text.contains("### F1 · Leak"));
        assert!(text.contains("a.rs:9"));
        assert!(text.contains("Reviewer C (challenges): Freed in drop."));
        assert!(text.contains("Severity they would give: low"));
        assert!(!text.contains("F2"));
    }

    #[test]
    fn the_chair_is_told_to_weigh_evidence_and_its_own_findings_alike() {
        let review = review();
        let text = verdict(&review, &review.reviewers[0]);
        assert!(text.contains("same scrutiny"));
        assert!(text.contains("No reviewer reported a finding."));
        assert!(text.contains("\"verdict\""));
    }
}
