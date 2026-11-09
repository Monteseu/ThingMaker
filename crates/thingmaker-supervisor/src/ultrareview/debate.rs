//! The rules of the debate, as plain functions over a [`Review`]: who is asked
//! about what, what each answer does to a finding, and when there is nothing
//! left to argue. No I/O here, so every rule is tested on its own.

use super::{
    model::{Entry, EntryKind, Finding, Outcome, Recommendation, Review, Severity, Standing, USER, Verdict, VerdictItem},
    parse::{Action, RawFinding, RawRebuttal, RawReply, RawVerdict, Stance},
};

/// Adds a reviewer's blind findings, numbered after the ones already in.
pub fn add_findings(review: &mut Review, author: &str, raw: Vec<RawFinding>, at: u64) {
    for finding in raw {
        let id = format!("F{}", review.findings.len() + 1);
        review.findings.push(Finding {
            id,
            author: author.to_string(),
            title: finding.title,
            severity: finding.severity,
            file: finding.file,
            line: finding.line,
            thread: vec![Entry { by: author.to_string(), kind: EntryKind::Claim, text: finding.claim, evidence: finding.evidence, severity: Some(finding.severity), fix: finding.fix, round: 0, at }],
            standing: Standing::Open,
            settled: false,
        });
    }
}

fn is_stance(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::Confirm | EntryKind::Challenge | EntryKind::Refine)
}

/// The findings a reviewer answers in cross-examination `round`: everyone
/// else's that are still argued, and, after the first round, only those whose
/// thread moved since its own last word.
pub fn asked_in_cross(review: &Review, reviewer: &str, round: u8) -> Vec<String> {
    review
        .findings
        .iter()
        .filter(|finding| finding.author != reviewer && !finding.settled && finding.standing != Standing::Withdrawn)
        .filter(|finding| {
            if round <= 1 {
                return true;
            }
            match finding.thread.iter().rposition(|entry| entry.by == reviewer && is_stance(entry.kind)) {
                // Never answered (it joined late, or its reply was lost).
                None => true,
                Some(last) => finding.thread[last + 1..].iter().any(|entry| entry.by != reviewer),
            }
        })
        .map(|finding| finding.id.clone())
        .collect()
}

/// Records cross-examination replies. A reviewer never judges its own finding.
pub fn apply_replies(review: &mut Review, reviewer: &str, replies: Vec<RawReply>, round: u8, at: u64) {
    for reply in replies {
        let Some(finding) = review.findings.iter_mut().find(|finding| finding.id == reply.finding) else { continue };
        if finding.author == reviewer {
            continue;
        }
        let kind = match reply.stance {
            Stance::Confirm => EntryKind::Confirm,
            Stance::Challenge => EntryKind::Challenge,
            Stance::Refine => EntryKind::Refine,
        };
        finding.thread.push(Entry { by: reviewer.to_string(), kind, text: reply.argument, evidence: reply.evidence, severity: reply.severity, fix: reply.fix, round, at });
    }
    settle(review);
}

/// The findings an author answers in rebuttal `round`: its own, challenged or
/// refined in that round.
pub fn asked_in_rebuttal(review: &Review, author: &str, round: u8) -> Vec<String> {
    review
        .findings
        .iter()
        .filter(|finding| finding.author == author && finding.standing != Standing::Withdrawn && !finding.settled)
        .filter(|finding| finding.thread.iter().any(|entry| entry.round == round && matches!(entry.kind, EntryKind::Challenge | EntryKind::Refine)))
        .map(|finding| finding.id.clone())
        .collect()
}

pub fn apply_rebuttals(review: &mut Review, author: &str, rebuttals: Vec<RawRebuttal>, round: u8, at: u64) {
    for rebuttal in rebuttals {
        let Some(finding) = review.findings.iter_mut().find(|finding| finding.id == rebuttal.finding && finding.author == author) else { continue };
        let kind = match rebuttal.action {
            Action::Defend => EntryKind::Defend,
            Action::Accept => EntryKind::Accept,
            Action::Withdraw => EntryKind::Withdraw,
        };
        if rebuttal.action == Action::Accept
            && let Some(severity) = rebuttal.severity
        {
            finding.severity = severity;
        }
        let text = if rebuttal.argument.is_empty() { "Withdrawn.".to_string() } else { rebuttal.argument };
        finding.thread.push(Entry { by: author.to_string(), kind, text, evidence: rebuttal.evidence, severity: rebuttal.severity, fix: rebuttal.fix, round, at });
    }
    settle(review);
}

/// The user's word on a finding, for the reviewers' next turns and the Chair.
pub fn add_note(review: &mut Review, finding: &str, text: &str, at: u64) -> bool {
    let round = review.round;
    let Some(finding) = review.findings.iter_mut().find(|candidate| candidate.id == finding) else { return false };
    finding.thread.push(Entry { by: USER.to_string(), kind: EntryKind::Note, text: text.trim().to_string(), evidence: None, severity: None, fix: None, round, at });
    // A settled finding the user reopens is argued again.
    finding.settled = false;
    true
}

/// Each reviewer's latest stance on a finding, the author's aside.
fn stances(finding: &Finding) -> Vec<(String, EntryKind)> {
    let mut latest: Vec<(String, EntryKind)> = Vec::new();
    for entry in &finding.thread {
        if entry.by == finding.author || !is_stance(entry.kind) {
            continue;
        }
        match latest.iter_mut().find(|(by, _)| *by == entry.by) {
            Some(slot) => slot.1 = entry.kind,
            None => latest.push((entry.by.clone(), entry.kind)),
        }
    }
    latest
}

/// Works out where every finding stands, and which are settled: withdrawn,
/// or answered by every other reviewer present, all the same way, with no
/// word from the user since.
pub fn settle(review: &mut Review) {
    let present: Vec<String> = review.present().map(|reviewer| reviewer.id.clone()).collect();
    for finding in &mut review.findings {
        if finding.thread.iter().any(|entry| entry.by == finding.author && entry.kind == EntryKind::Withdraw) {
            finding.standing = Standing::Withdrawn;
            finding.settled = true;
            continue;
        }
        let latest = stances(finding);
        let agree = latest.iter().filter(|(_, kind)| matches!(kind, EntryKind::Confirm | EntryKind::Refine)).count();
        let against = latest.iter().filter(|(_, kind)| *kind == EntryKind::Challenge).count();
        finding.standing = match (agree, against) {
            (0, 0) => Standing::Open,
            (_, 0) => Standing::Confirmed,
            (0, _) => Standing::Challenged,
            _ => Standing::Contested,
        };
        let everyone = present.iter().filter(|id| **id != finding.author).all(|id| latest.iter().any(|(by, _)| by == id));
        let user_last = finding.thread.last().is_some_and(|entry| entry.by == USER);
        // Unanimous challenges still wait for the author's answer.
        let answered = finding.standing != Standing::Challenged || finding.thread.last().is_some_and(|entry| entry.by == finding.author);
        finding.settled = everyone && !user_last && matches!(finding.standing, Standing::Confirmed | Standing::Challenged) && answered;
    }
}

pub fn all_settled(review: &Review) -> bool {
    review.findings.iter().all(|finding| finding.settled)
}

/// What the debate itself says about a finding, for a verdict the Chair left
/// a finding out of, or could not write at all.
fn debate_item(finding: &Finding) -> VerdictItem {
    let outcome = match finding.standing {
        Standing::Withdrawn => Outcome::Withdrawn,
        Standing::Confirmed => Outcome::Confirmed,
        Standing::Challenged => Outcome::Refuted,
        Standing::Contested => Outcome::Split,
        // Nobody answered: it stands on its author's word, which the user weighs.
        Standing::Open => Outcome::Split,
    };
    let claim = finding.thread.first();
    let challenges: Vec<&Entry> = finding.thread.iter().filter(|entry| entry.kind == EntryKind::Challenge).collect();
    VerdictItem {
        finding: finding.id.clone(),
        outcome,
        severity: Some(finding.severity),
        summary: claim.map(|entry| entry.text.clone()).unwrap_or_default(),
        fix: finding.thread.iter().rev().find_map(|entry| entry.fix.clone()),
        case_for: (outcome == Outcome::Split).then(|| claim.map(|entry| entry.text.clone()).unwrap_or_default()),
        against: (outcome == Outcome::Split).then(|| challenges.last().map(|entry| entry.text.clone()).unwrap_or_else(|| "No other reviewer answered it.".into())),
        decision: None,
    }
}

fn recommendation_for(items: &[VerdictItem]) -> Recommendation {
    let confirmed: Vec<&VerdictItem> = items.iter().filter(|item| item.outcome == Outcome::Confirmed).collect();
    if confirmed.iter().any(|item| item.severity.is_some_and(|severity| severity >= Severity::High)) {
        Recommendation::RequestChanges
    } else if confirmed.is_empty() && items.iter().all(|item| item.outcome != Outcome::Split) {
        Recommendation::Approve
    } else {
        Recommendation::ApproveWithFixes
    }
}

/// The Chair's verdict, with every finding it left out filled in from the debate.
pub fn apply_verdict(review: &mut Review, raw: RawVerdict) {
    let mut items = Vec::new();
    for finding in &review.findings {
        match raw.items.iter().find(|item| item.finding == finding.id) {
            Some(item) => items.push(VerdictItem {
                finding: item.finding.clone(),
                // A withdrawal stays one, whatever the Chair wrote.
                outcome: if finding.standing == Standing::Withdrawn { Outcome::Withdrawn } else { item.outcome },
                severity: item.severity.or(Some(finding.severity)),
                summary: if item.summary.is_empty() { debate_item(finding).summary } else { item.summary.clone() },
                fix: item.fix.clone().or_else(|| debate_item(finding).fix),
                case_for: item.case_for.clone(),
                against: item.against.clone(),
                decision: None,
            }),
            None => items.push(debate_item(finding)),
        }
    }
    review.verdict = Some(Verdict { recommendation: raw.recommendation, headline: raw.headline, summary: raw.summary, items, by_chair: true });
}

/// A verdict summed up from the debate alone, when the Chair could not write one.
pub fn fallback_verdict(review: &mut Review) {
    let items: Vec<VerdictItem> = review.findings.iter().map(debate_item).collect();
    let confirmed = items.iter().filter(|item| item.outcome == Outcome::Confirmed).count();
    let split = items.iter().filter(|item| item.outcome == Outcome::Split).count();
    let headline = match (confirmed, split) {
        (0, 0) => "No findings held up".to_string(),
        (c, 0) => format!("{c} finding{} held up", if c == 1 { "" } else { "s" }),
        (c, s) => format!("{c} confirmed, {s} for you to decide"),
    };
    review.verdict = Some(Verdict {
        recommendation: recommendation_for(&items),
        headline,
        summary: "The Chair could not write the verdict, so this one is summed up from where each finding stood when the debate ended.".into(),
        items,
        by_chair: false,
    });
}

/// How each reviewer's findings held up in one finished review, by seat.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRecord {
    pub provider: crate::agents::Provider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub lens: super::model::Lens,
    pub raised: u32,
    pub confirmed: u32,
    pub reviews: u32,
}

/// Every finished review's results, summed by provider, model and lens.
pub fn track_record(reviews: &[Review]) -> Vec<SeatRecord> {
    let mut records: Vec<SeatRecord> = Vec::new();
    for review in reviews.iter().filter(|review| review.verdict.is_some()) {
        let verdict = review.verdict.as_ref().expect("filtered");
        for reviewer in &review.reviewers {
            let raised: Vec<&Finding> = review.findings.iter().filter(|finding| finding.author == reviewer.id).collect();
            let confirmed = raised.iter().filter(|finding| verdict.items.iter().any(|item| item.finding == finding.id && item.outcome == Outcome::Confirmed)).count();
            let slot = records.iter_mut().find(|record| record.provider == reviewer.provider && record.model == reviewer.model && record.lens == reviewer.lens);
            match slot {
                Some(record) => {
                    record.raised += raised.len() as u32;
                    record.confirmed += confirmed as u32;
                    record.reviews += 1;
                }
                None => records.push(SeatRecord { provider: reviewer.provider, model: reviewer.model.clone(), lens: reviewer.lens, raised: raised.len() as u32, confirmed: confirmed as u32, reviews: 1 }),
            }
        }
    }
    records
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::{
        agents::Provider,
        ultrareview::model::{Config, Lens, Phase, Review, Reviewer, Status, Target, TargetKind},
    };

    pub fn review(seats: usize) -> Review {
        Review {
            id: "r1".into(),
            workspace_id: "w".into(),
            host: "s".into(),
            target: Target { kind: TargetKind::Branch, number: None, title: "t".into(), url: None, base_ref: "main".into(), head_ref: "x".into(), base_sha: "a".into(), head_sha: "b".into(), files: 1, additions: 1, deletions: 0 },
            config: Config { quick: false, rounds: 2, require_evidence: true, allow_tests: true },
            reviewers: (0..seats)
                .map(|index| Reviewer { id: crate::ultrareview::model::letter(index), provider: Provider::Claude, model: None, effort: None, lens: Lens::General, focus: None, chair: index == 0, absent: false, done: None, last_job: None })
                .collect(),
            findings: Vec::new(),
            verdict: None,
            phase: Phase::Blind,
            round: 1,
            status: Status::Running,
            error: None,
            root: "/tmp".into(),
            worktree_id: None,
            checkout_workspace: None,
            log: Vec::new(),
            created_at: 0,
            updated_at: 0,
            finished_at: None,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use super::tests_support::review;
    use crate::{agents::Provider, ultrareview::model::Lens};

    fn raw(title: &str) -> RawFinding {
        RawFinding { title: title.into(), severity: Severity::Medium, file: Some("a.rs".into()), line: Some(3), claim: format!("{title} is wrong"), evidence: None, fix: None }
    }

    fn reply(finding: &str, stance: Stance) -> RawReply {
        RawReply { finding: finding.into(), stance, argument: "because".into(), evidence: None, severity: None, fix: None }
    }

    #[test]
    fn findings_are_numbered_in_the_order_they_come_in() {
        let mut review = review(3);
        add_findings(&mut review, "A", vec![raw("one"), raw("two")], 1);
        add_findings(&mut review, "B", vec![raw("three")], 2);
        assert_eq!(review.findings.iter().map(|f| (f.id.as_str(), f.author.as_str())).collect::<Vec<_>>(), [("F1", "A"), ("F2", "A"), ("F3", "B")]);
        assert_eq!(review.findings[0].thread[0].kind, EntryKind::Claim);
    }

    #[test]
    fn nobody_is_asked_about_or_judges_their_own_finding() {
        let mut review = review(3);
        add_findings(&mut review, "A", vec![raw("one")], 1);
        add_findings(&mut review, "B", vec![raw("two")], 1);
        assert_eq!(asked_in_cross(&review, "A", 1), ["F2"]);
        assert_eq!(asked_in_cross(&review, "C", 1), ["F1", "F2"]);
        apply_replies(&mut review, "A", vec![reply("F1", Stance::Confirm), reply("F2", Stance::Confirm)], 1, 2);
        assert_eq!(review.findings[0].thread.len(), 1, "its own finding is untouched");
        assert_eq!(review.findings[1].thread.len(), 2);
    }

    #[test]
    fn standing_follows_each_reviewers_latest_word() {
        let mut review = review(3);
        add_findings(&mut review, "A", vec![raw("one")], 1);
        apply_replies(&mut review, "B", vec![reply("F1", Stance::Confirm)], 1, 2);
        assert_eq!(review.findings[0].standing, Standing::Confirmed);
        assert!(!review.findings[0].settled, "C has not answered");
        apply_replies(&mut review, "C", vec![reply("F1", Stance::Challenge)], 1, 3);
        assert_eq!(review.findings[0].standing, Standing::Contested);
        // C changes its mind in the next round.
        apply_replies(&mut review, "C", vec![reply("F1", Stance::Refine)], 2, 4);
        assert_eq!(review.findings[0].standing, Standing::Confirmed);
        assert!(review.findings[0].settled);
        assert!(all_settled(&review));
    }

    #[test]
    fn a_unanimous_challenge_waits_for_the_author_and_a_withdrawal_settles_it() {
        let mut review = review(2);
        add_findings(&mut review, "A", vec![raw("one")], 1);
        apply_replies(&mut review, "B", vec![reply("F1", Stance::Challenge)], 1, 2);
        assert_eq!(review.findings[0].standing, Standing::Challenged);
        assert!(!review.findings[0].settled);
        assert_eq!(asked_in_rebuttal(&review, "A", 1), ["F1"]);
        apply_rebuttals(&mut review, "A", vec![RawRebuttal { finding: "F1".into(), action: Action::Withdraw, argument: String::new(), evidence: None, severity: None, fix: None }], 1, 3);
        assert_eq!(review.findings[0].standing, Standing::Withdrawn);
        assert!(review.findings[0].settled);
        assert_eq!(review.findings[0].thread.last().unwrap().text, "Withdrawn.");
    }

    #[test]
    fn later_rounds_only_ask_about_threads_that_moved() {
        let mut review = review(3);
        add_findings(&mut review, "A", vec![raw("one"), raw("two")], 1);
        apply_replies(&mut review, "B", vec![reply("F1", Stance::Challenge), reply("F2", Stance::Confirm)], 1, 2);
        apply_replies(&mut review, "C", vec![reply("F1", Stance::Confirm), reply("F2", Stance::Confirm)], 1, 2);
        // F2 is settled; F1 is contested and its author defends it.
        apply_rebuttals(&mut review, "A", vec![RawRebuttal { finding: "F1".into(), action: Action::Defend, argument: "Here is the test.".into(), evidence: Some("cargo test x".into()), severity: None, fix: None }], 1, 3);
        assert_eq!(asked_in_cross(&review, "B", 2), ["F1"]);
        assert_eq!(asked_in_cross(&review, "C", 2), ["F1"]);
    }

    #[test]
    fn accepting_a_refinement_takes_its_severity_and_a_user_note_reopens_a_finding() {
        let mut review = review(2);
        add_findings(&mut review, "A", vec![raw("one")], 1);
        apply_replies(&mut review, "B", vec![RawReply { severity: Some(Severity::Low), ..reply("F1", Stance::Refine) }], 1, 2);
        assert!(review.findings[0].settled);
        apply_rebuttals(&mut review, "A", vec![RawRebuttal { finding: "F1".into(), action: Action::Accept, argument: "Fair.".into(), evidence: None, severity: Some(Severity::Low), fix: None }], 1, 3);
        assert_eq!(review.findings[0].severity, Severity::Low);
        assert!(add_note(&mut review, "F1", " It ships to every user. ", 4));
        settle(&mut review);
        assert!(!review.findings[0].settled, "the user spoke last");
        assert!(!add_note(&mut review, "F9", "x", 5));
    }

    #[test]
    fn the_verdict_fills_in_what_the_chair_left_out_and_keeps_withdrawals() {
        let mut review = review(2);
        add_findings(&mut review, "A", vec![raw("one"), raw("two"), raw("three")], 1);
        apply_replies(&mut review, "B", vec![reply("F1", Stance::Confirm), reply("F2", Stance::Challenge), reply("F3", Stance::Challenge)], 1, 2);
        apply_rebuttals(&mut review, "A", vec![RawRebuttal { finding: "F3".into(), action: Action::Withdraw, argument: String::new(), evidence: None, severity: None, fix: None }], 1, 3);
        let raw = RawVerdict {
            recommendation: Recommendation::ApproveWithFixes,
            headline: "One fix".into(),
            summary: String::new(),
            items: vec![crate::ultrareview::parse::RawItem { finding: "F3".into(), outcome: Outcome::Confirmed, severity: None, summary: "x".into(), fix: None, case_for: None, against: None }],
        };
        apply_verdict(&mut review, raw);
        let verdict = review.verdict.as_ref().unwrap();
        assert!(verdict.by_chair);
        assert_eq!(verdict.items.iter().map(|item| item.outcome).collect::<Vec<_>>(), [Outcome::Confirmed, Outcome::Refuted, Outcome::Withdrawn]);
    }

    #[test]
    fn without_a_chair_the_debate_sums_itself_up() {
        let mut review = review(3);
        add_findings(&mut review, "A", vec![raw("one"), raw("two")], 1);
        apply_replies(&mut review, "B", vec![RawReply { severity: Some(Severity::High), ..reply("F1", Stance::Confirm) }, reply("F2", Stance::Confirm)], 1, 2);
        apply_replies(&mut review, "C", vec![reply("F1", Stance::Confirm), reply("F2", Stance::Challenge)], 1, 2);
        fallback_verdict(&mut review);
        let verdict = review.verdict.unwrap();
        assert!(!verdict.by_chair);
        assert_eq!(verdict.items[1].outcome, Outcome::Split);
        assert!(verdict.items[1].against.is_some());
        assert_eq!(verdict.headline, "1 confirmed, 1 for you to decide");
    }

    #[test]
    fn the_track_record_sums_seats_across_reviews() {
        let mut one = review(2);
        one.reviewers[1].provider = Provider::Codex;
        one.reviewers[1].lens = Lens::Correctness;
        add_findings(&mut one, "B", vec![raw("one"), raw("two")], 1);
        apply_replies(&mut one, "A", vec![reply("F1", Stance::Confirm), reply("F2", Stance::Challenge)], 1, 2);
        fallback_verdict(&mut one);
        let mut two = one.clone();
        two.id = "r2".into();
        let records = track_record(&[one, two, review(2)]);
        let codex = records.iter().find(|record| record.provider == Provider::Codex).unwrap();
        assert_eq!((codex.raised, codex.confirmed, codex.reviews), (4, 2, 2));
    }
}
