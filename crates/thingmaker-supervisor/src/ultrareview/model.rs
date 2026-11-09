//! What an Ultra-Review is made of: the change under review, the panel, the
//! findings and their threads, and the verdict. Kept whole in one record per
//! review (`storage::ultrareview`), and sent whole to the renderer on every
//! change: a review holds a few dozen entries, not thousands.

use serde::{Deserialize, Serialize};

use crate::agents::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "low" | "minor" | "nit" => Some(Self::Low),
            "medium" | "moderate" | "normal" => Some(Self::Medium),
            "high" | "major" => Some(Self::High),
            "critical" | "blocker" | "severe" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// A GitHub pull request, checked out from its head.
    Pr,
    /// The workspace's current branch against its base, uncommitted changes included.
    Branch,
}

/// The change under review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub kind: TargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub base_ref: String,
    pub head_ref: String,
    /// The commit the change is measured from (the merge base).
    pub base_sha: String,
    pub head_sha: String,
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
}

/// What a reviewer looks for. The panel's lenses keep reviewers from all
/// finding the same three things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lens {
    Architecture,
    Correctness,
    Tests,
    Security,
    Performance,
    Readability,
    /// Argues against the strongest claims, so nothing passes on agreement alone.
    DevilsAdvocate,
    General,
}

impl Lens {
    /// What the reviewer is told its lens is, in the prompt.
    pub fn brief(self) -> &'static str {
        match self {
            Self::Architecture => "architecture and design: structure, boundaries, coupling, whether the change fits the codebase and will age well",
            Self::Correctness => "correctness: logic errors, edge cases, state and concurrency, error handling. Back every bug you claim with a failing test or a concrete input",
            Self::Tests => "tests: what the change's tests really prove, what they miss, and whether they would catch a regression",
            Self::Security => "security: injection, secrets, permissions, unsafe input handling, data exposure",
            Self::Performance => "performance: work done per call or render, timers, memory, I/O, anything that grows with the data",
            Self::Readability => "readability and maintainability: naming, clarity, dead code, comments that mislead",
            Self::DevilsAdvocate => "devil's advocate: look hardest at what everyone would agree on, and argue against claims that are weaker than they look. Disagree when the evidence is thin, not by reflex",
            Self::General => "a general review: anything that matters to the author",
        }
    }
}

/// One seat on the panel. Reviewers know each other only by `id`, a letter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    /// "A", "B", …: the only name other reviewers ever see.
    pub id: String,
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub lens: Lens,
    /// The user's own words on what this reviewer should look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// Runs the debate's verdict. Exactly one reviewer is the Chair.
    #[serde(default)]
    pub chair: bool,
    /// Dropped out: its turns kept failing, so the review goes on without it.
    #[serde(default)]
    pub absent: bool,
    /// The last step this reviewer finished (`blind`, `cross:1`, `rebuttal:1`,
    /// `verdict`), so a resumed review does not ask it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// Its last job, for a follow-up in the same worker session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_job: Option<String>,
}

impl Reviewer {
    /// Its name as a delegation worker, and in the sidebar.
    pub fn worker_name(&self) -> String {
        format!("Reviewer {}", self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// One round of cross-examination, no rebuttal, and no tests run.
    #[serde(default)]
    pub quick: bool,
    /// Rounds of cross-examination and rebuttal (1 to 3); ignored when quick.
    pub rounds: u8,
    /// A claim or a challenge needs evidence: a file and line, a test, a repro.
    pub require_evidence: bool,
    /// Reviewers may run tests and builds in their checkout.
    pub allow_tests: bool,
}

impl Config {
    pub fn cycles(&self) -> u8 {
        if self.quick { 1 } else { self.rounds.clamp(1, 3) }
    }

    pub fn runs_tests(&self) -> bool {
        self.allow_tests && !self.quick
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// The finding as its author raised it.
    Claim,
    Confirm,
    Challenge,
    Refine,
    /// The author's answers, in rebuttal.
    Defend,
    Accept,
    Withdraw,
    /// The user stepped in.
    Note,
}

/// One message in a finding's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// A reviewer's letter, or `user`.
    pub by: String,
    pub kind: EntryKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// 0 for the blind review, then the cross-examination cycle.
    pub round: u8,
    pub at: u64,
}

pub const USER: &str = "user";

/// Where a finding stands during the debate (the verdict decides in the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Nobody has answered it yet.
    Open,
    /// Everyone who answered agrees it is real.
    Confirmed,
    /// Some agree, some challenge it.
    Contested,
    /// Everyone who answered challenges it.
    Challenged,
    /// Its author took it back.
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// "F1", "F2", …
    pub id: String,
    /// The author's letter.
    pub author: String,
    pub title: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Starts with the claim.
    pub thread: Vec<Entry>,
    pub standing: Standing,
    /// Nothing more to argue: withdrawn, or everyone agrees.
    #[serde(default)]
    pub settled: bool,
}

impl Finding {
    pub fn location(&self) -> Option<String> {
        self.file.as_ref().map(|file| match self.line {
            Some(line) => format!("{file}:{line}"),
            None => file.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Confirmed,
    Refuted,
    /// A real disagreement the evidence can't settle: the user decides.
    Split,
    Withdrawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Fix,
    Leave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictItem {
    pub finding: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// For a split: the best case that it is real, and that it is not.
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub case_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub against: Option<String>,
    /// The user's call on a split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recommendation {
    Approve,
    ApproveWithFixes,
    RequestChanges,
}

impl Recommendation {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "approve" => Some(Self::Approve),
            "approve_with_fixes" | "approve_with_changes" => Some(Self::ApproveWithFixes),
            "request_changes" | "changes_requested" | "reject" => Some(Self::RequestChanges),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub recommendation: Recommendation,
    pub headline: String,
    pub summary: String,
    pub items: Vec<VerdictItem>,
    /// Written by the Chair; `false` when ThingMaker had to sum the debate up
    /// itself because the Chair could not.
    pub by_chair: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Blind,
    Cross,
    Rebuttal,
    Verdict,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Done,
    Failed,
    Cancelled,
    /// ThingMaker closed while it ran: it can be resumed.
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub workspace_id: String,
    /// The agent's id for the session the review belongs to.
    pub host: String,
    pub target: Target,
    pub config: Config,
    pub reviewers: Vec<Reviewer>,
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    pub phase: Phase,
    /// The cross-examination cycle, from 1.
    pub round: u8,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Where the reviewers work: a checkout of the change made for the review.
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// The workspace record the checkout was registered as, forgotten afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_workspace: Option<String>,
    /// Things worth telling the user that are not findings: a reviewer that
    /// dropped out, a reply that could not be read.
    #[serde(default)]
    pub log: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

impl Review {
    pub fn reviewer(&self, id: &str) -> Option<&Reviewer> {
        self.reviewers.iter().find(|reviewer| reviewer.id == id)
    }

    pub fn chair(&self) -> Option<&Reviewer> {
        self.reviewers.iter().find(|reviewer| reviewer.chair)
    }

    pub fn present(&self) -> impl Iterator<Item = &Reviewer> {
        self.reviewers.iter().filter(|reviewer| !reviewer.absent)
    }

    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|finding| finding.id == id)
    }

    /// The key the review's reviewers are registered under with delegation.
    pub fn orchestrator_key(&self) -> String {
        orchestrator_key(&self.id)
    }

    pub fn is_live(&self) -> bool {
        self.status == Status::Running
    }
}

pub fn orchestrator_key(review_id: &str) -> String {
    format!("ultra-review:{review_id}")
}

/// The letters reviewers are known by.
pub fn letter(index: usize) -> String {
    char::from(b'A' + (index % 26) as u8).to_string()
}
