//! The verdict as Markdown: what gets posted on the pull request, or copied.
//! Reviewers are named here, once the debate is over: the author of the
//! change should know who looked at it.

use super::model::{Decision, Lens, Outcome, Recommendation, Review, Severity};

fn lens(lens: Lens) -> &'static str {
    match lens {
        Lens::Architecture => "architecture",
        Lens::Correctness => "correctness",
        Lens::Tests => "tests",
        Lens::Security => "security",
        Lens::Performance => "performance",
        Lens::Readability => "readability",
        Lens::DevilsAdvocate => "devil's advocate",
        Lens::General => "general",
    }
}

fn severity(severity: Option<Severity>) -> &'static str {
    severity.map(Severity::as_str).unwrap_or("medium")
}

pub fn render(review: &Review) -> String {
    let Some(verdict) = &review.verdict else {
        return "The review has no verdict yet.".into();
    };
    let recommendation = match verdict.recommendation {
        Recommendation::Approve => "Approve",
        Recommendation::ApproveWithFixes => "Approve after fixes",
        Recommendation::RequestChanges => "Request changes",
    };
    let mut text = format!("## Ultra-Review: {}\n\n**{recommendation}.** {}\n", verdict.headline, verdict.summary.trim());
    let section = |title: &str, outcome: Outcome, text: &mut String| {
        let items: Vec<_> = verdict.items.iter().filter(|item| item.outcome == outcome).collect();
        if items.is_empty() {
            return;
        }
        text.push_str(&format!("\n### {title}\n\n"));
        for item in items {
            let Some(finding) = review.finding(&item.finding) else { continue };
            let location = finding.location().map(|location| format!(" (`{location}`)")).unwrap_or_default();
            text.push_str(&format!("- **[{}] {}**{location}\n  {}\n", severity(item.severity), finding.title, item.summary.trim().replace('\n', "\n  ")));
            if let Some(fix) = item.fix.as_deref().filter(|_| outcome == Outcome::Confirmed) {
                text.push_str(&format!("  Suggested fix: {}\n", fix.trim().replace('\n', "\n  ")));
            }
            if outcome == Outcome::Split {
                if let Some(case) = &item.case_for {
                    text.push_str(&format!("  For: {}\n", case.trim()));
                }
                if let Some(case) = &item.against {
                    text.push_str(&format!("  Against: {}\n", case.trim()));
                }
                match item.decision {
                    Some(Decision::Fix) => text.push_str("  Decision: fix it.\n"),
                    Some(Decision::Leave) => text.push_str("  Decision: leave it.\n"),
                    None => {}
                }
            }
        }
    };
    section("Confirmed", Outcome::Confirmed, &mut text);
    section("Needs a decision", Outcome::Split, &mut text);
    section("Refuted", Outcome::Refuted, &mut text);
    let withdrawn = verdict.items.iter().filter(|item| item.outcome == Outcome::Withdrawn).count();
    if withdrawn > 0 {
        text.push_str(&format!("\n{withdrawn} more finding{} raised and withdrawn during the debate.\n", if withdrawn == 1 { " was" } else { "s were" }));
    }
    let panel: Vec<String> = review
        .reviewers
        .iter()
        .map(|reviewer| {
            let model = reviewer.model.clone().unwrap_or_else(|| reviewer.provider.label().to_string());
            let role = if reviewer.chair { format!("chair, {}", lens(reviewer.lens)) } else { lens(reviewer.lens).to_string() };
            format!("{model} ({role})")
        })
        .collect();
    text.push_str(&format!("\n---\n_Cross-reviewed by {} with ThingMaker Ultra-Review: {}._\n", panel.len(), panel.join(", ")));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ultrareview::{
        debate::{add_findings, apply_replies, fallback_verdict, tests_support::review},
        parse::{RawFinding, RawReply, Stance},
    };

    #[test]
    fn the_write_up_groups_findings_and_names_the_panel() {
        let mut review = review(2);
        review.reviewers[0].model = Some("claude-opus-5-5".into());
        add_findings(&mut review, "B", vec![RawFinding { title: "Leak".into(), severity: Severity::High, file: Some("a.rs".into()), line: Some(2), claim: "x".into(), evidence: None, fix: Some("drop it".into()) }], 1);
        apply_replies(&mut review, "A", vec![RawReply { finding: "F1".into(), stance: Stance::Confirm, argument: "yes".into(), evidence: None, severity: None, fix: None }], 1, 2);
        fallback_verdict(&mut review);
        let text = render(&review);
        assert!(text.starts_with("## Ultra-Review: 1 finding held up"));
        assert!(text.contains("**Request changes.**"));
        assert!(text.contains("- **[high] Leak** (`a.rs:2`)"));
        assert!(text.contains("Suggested fix: drop it"));
        assert!(text.contains("claude-opus-5-5 (chair, general)"));
    }
}
