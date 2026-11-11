//! Runs a review: phase by phase, each reviewer's turn in parallel, every
//! answer recorded as it comes in, and the checkout removed at the end.
//!
//! Reviewers are delegation workers under their own orchestrator key
//! (`ultra-review:<id>`), pinned by name, so a reviewer whose account hits a
//! limit waits it out instead of being swapped for another (`Runner`). Each
//! turn ends in a JSON block; one that cannot be read is asked for once more,
//! and a reviewer whose turns keep failing drops out rather than stalling the
//! review.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::Deserialize;

use super::{
    checkout::{self, Checkout},
    debate,
    model::{Config, Decision, Lens, Phase, Review, Reviewer, Status, TargetKind, letter},
    parse::{self, RawFinding, RawRebuttal, RawReply, RawVerdict},
    prompt,
};
use crate::{
    agents::Provider,
    delegation::{
        Combo, Delegation, JobStatus, WorkerSlot,
        jobs::{BoxFuture, DelegateArgs, now_unix_ms},
    },
    error::DesktopError,
    storage::Storage,
};

/// One reviewer turn, done.
#[derive(Debug, Clone)]
pub struct Turn {
    pub job: String,
    pub text: String,
}

/// How reviewers' turns are run. [`DelegationRunner`] in the app; a script in tests.
pub trait Runner: Send + Sync {
    /// Seats the panel before its first turn (and again on resume).
    fn open(&self, review: &Review) -> Result<(), String>;
    /// One turn. `follow_up` continues the reviewer's previous job in the
    /// same session when it is still open.
    fn turn(&self, review_id: &str, reviewer: &Reviewer, task: String, follow_up: Option<String>) -> BoxFuture<Result<Turn, String>>;
    /// Stops every turn still running and lets the panel go.
    fn close(&self, review_id: &str) -> BoxFuture<()>;
}

/// Reviewers as delegation workers.
pub struct DelegationRunner {
    pub delegation: Delegation,
}

impl DelegationRunner {
    fn slot(reviewer: &Reviewer) -> WorkerSlot {
        WorkerSlot { name: reviewer.worker_name(), provider: reviewer.provider, model: reviewer.model.clone(), effort: reviewer.effort.clone(), capabilities: Vec::new(), note: None, prompt: None }
    }
}

impl Runner for DelegationRunner {
    fn open(&self, review: &Review) -> Result<(), String> {
        // The registered lead stands in for the review itself. A local one
        // keeps its team local, which is right only for an all-local panel:
        // a local Chair beside cloud reviewers must not lock them out.
        let all_local = review.reviewers.iter().all(|reviewer| reviewer.provider == Provider::Local);
        let lead = match review.chair().map(|chair| chair.provider) {
            Some(Provider::Local) if !all_local => Provider::Claude,
            Some(provider) => provider,
            None => Provider::Claude,
        };
        let combo = Combo { workers: review.reviewers.iter().map(Self::slot).collect(), native_subagents: false };
        self.delegation.register(&review.orchestrator_key(), lead, PathBuf::from(&review.root), combo);
        Ok(())
    }

    fn turn(&self, review_id: &str, reviewer: &Reviewer, task: String, follow_up: Option<String>) -> BoxFuture<Result<Turn, String>> {
        let delegation = self.delegation.clone();
        let key = super::model::orchestrator_key(review_id);
        let worker = reviewer.worker_name();
        Box::pin(async move {
            let args = |continue_job: Option<String>| DelegateArgs { task: task.clone(), worker: Some(worker.clone()), capability: None, files: Vec::new(), continue_job, pinned: true };
            // A follow-up keeps what the reviewer has read; its session may
            // have closed in the meantime, and then a fresh one starts.
            let job = match follow_up {
                Some(previous) => delegation.delegate(&key, args(Some(previous))).or_else(|_| delegation.delegate(&key, args(None))),
                None => delegation.delegate(&key, args(None)),
            }?;
            loop {
                let jobs = delegation.await_jobs(&key, std::slice::from_ref(&job.id), Duration::from_secs(600), false).await?;
                let Some(done) = jobs.into_iter().find(|candidate| candidate.id == job.id) else {
                    return Err("the reviewer's job is gone".into());
                };
                match done.status {
                    JobStatus::Succeeded => return Ok(Turn { job: done.id, text: done.result.unwrap_or_default() }),
                    JobStatus::Failed | JobStatus::Cancelled => return Err(done.error.unwrap_or_else(|| "the reviewer's turn ended without a reply".into())),
                    // Still working, or waiting out a limit: keep waiting.
                    _ => continue,
                }
            }
        })
    }

    fn close(&self, review_id: &str) -> BoxFuture<()> {
        let delegation = self.delegation.clone();
        let key = super::model::orchestrator_key(review_id);
        Box::pin(async move { delegation.release(&key).await })
    }
}

/// A seat as the user set it up.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRequest {
    pub provider: Provider,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    pub lens: Lens,
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub chair: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub workspace_id: String,
    /// The agent's id for the session the review belongs to.
    pub host: String,
    pub kind: TargetKind,
    #[serde(default)]
    pub number: Option<u64>,
    pub seats: Vec<SeatRequest>,
    pub config: Config,
}

pub const MIN_SEATS: usize = 2;
pub const MAX_SEATS: usize = 6;

/// The reviewers for a request: lettered in order, with exactly one Chair
/// (the first seat, when none was chosen).
pub fn seat(request: &[SeatRequest]) -> Result<Vec<Reviewer>, String> {
    if request.len() < MIN_SEATS || request.len() > MAX_SEATS {
        return Err(format!("A panel has {MIN_SEATS} to {MAX_SEATS} reviewers."));
    }
    let chair = request.iter().position(|seat| seat.chair).unwrap_or(0);
    Ok(request
        .iter()
        .enumerate()
        .map(|(index, seat)| Reviewer {
            id: letter(index),
            provider: seat.provider,
            model: seat.model.clone().filter(|model| !model.trim().is_empty() && model != "default"),
            effort: seat.effort.clone().filter(|effort| !effort.trim().is_empty() && effort != "default"),
            lens: seat.lens,
            focus: seat.focus.clone().map(|focus| focus.trim().chars().take(500).collect::<String>()).filter(|focus| !focus.is_empty()),
            chair: index == chair,
            absent: false,
            done: None,
            last_job: None,
        })
        .collect())
}

/// What a turn was asked for, and so how its reply is read.
#[derive(Debug, Clone)]
enum Ask {
    Blind,
    Cross(Vec<String>),
    Rebuttal(Vec<String>),
    Verdict(Vec<String>),
}

enum Answer {
    Findings(Vec<RawFinding>),
    Replies(Vec<RawReply>),
    Rebuttals(Vec<RawRebuttal>),
    Verdict(RawVerdict),
}

fn read(ask: &Ask, text: &str) -> Result<Answer, String> {
    match ask {
        Ask::Blind => parse::findings(text).map(Answer::Findings),
        Ask::Cross(asked) => parse::replies(text, asked).map(Answer::Replies),
        Ask::Rebuttal(asked) => parse::rebuttals(text, asked).map(Answer::Rebuttals),
        Ask::Verdict(known) => parse::verdict(text, known).map(Answer::Verdict),
    }
}

/// How a turn ended.
enum Outcome {
    Answered { job: String, answer: Answer },
    /// It replied, twice, with nothing usable.
    Unreadable { job: String, error: String },
    /// The turn itself failed (a crash, a limit it could not wait out).
    Failed(String),
}

async fn run_turn(runner: Arc<dyn Runner>, review_id: String, reviewer: Reviewer, ask: Ask, task: String) -> Outcome {
    let first = match runner.turn(&review_id, &reviewer, task, reviewer.last_job.clone()).await {
        Ok(turn) => turn,
        Err(error) => return Outcome::Failed(error),
    };
    let error = match read(&ask, &first.text) {
        Ok(answer) => return Outcome::Answered { job: first.job, answer },
        Err(error) => error,
    };
    match runner.turn(&review_id, &reviewer, prompt::correction(&error), Some(first.job.clone())).await {
        Ok(second) => match read(&ask, &second.text) {
            Ok(answer) => Outcome::Answered { job: second.job, answer },
            Err(error) => Outcome::Unreadable { job: second.job, error },
        },
        Err(_) => Outcome::Unreadable { job: first.job, error },
    }
}

struct Inner {
    storage: Arc<Mutex<Storage>>,
    runner: Arc<dyn Runner>,
    emit: Box<dyn Fn(&Review) + Send + Sync>,
    data_dir: PathBuf,
    home: Option<PathBuf>,
    /// Running reviews and their stop flags.
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Ultra-Review's service. Cloning shares it.
#[derive(Clone)]
pub struct UltraReviewService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for UltraReviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UltraReviewService").finish_non_exhaustive()
    }
}

fn store(error: crate::storage::StorageError) -> DesktopError {
    DesktopError::io(error.to_string())
}

impl UltraReviewService {
    pub fn new(storage: Arc<Mutex<Storage>>, runner: Arc<dyn Runner>, data_dir: PathBuf, home: Option<PathBuf>, emit: impl Fn(&Review) + Send + Sync + 'static) -> Self {
        Self { inner: Arc::new(Inner { storage, runner, emit: Box::new(emit), data_dir, home, running: Mutex::new(HashMap::new()) }) }
    }

    fn with_storage<T>(&self, f: impl FnOnce(&Storage) -> T) -> T {
        let guard = self.inner.storage.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard)
    }

    fn save(&self, review: &mut Review) {
        review.updated_at = now_unix_ms();
        if let Err(error) = self.with_storage(|storage| storage.ultra_review_save(review)) {
            tracing::warn!(%error, "could not save an Ultra-Review");
        }
        (self.inner.emit)(review);
    }

    pub fn get(&self, id: &str) -> Result<Review, DesktopError> {
        self.with_storage(|storage| storage.ultra_review_get(id)).map_err(store)?.ok_or_else(|| DesktopError::not_ready("that review is gone"))
    }

    pub fn list(&self, host: &str) -> Result<Vec<Review>, DesktopError> {
        self.with_storage(|storage| storage.ultra_review_list(host)).map_err(store)
    }

    pub fn track_record(&self) -> Result<Vec<debate::SeatRecord>, DesktopError> {
        Ok(debate::track_record(&self.with_storage(|storage| storage.ultra_review_finished()).map_err(store)?))
    }

    pub fn gh(&self) -> Option<PathBuf> {
        checkout::find_gh(self.inner.home.as_deref())
    }

    /// The open pull requests of a workspace's repository, or the ones `query` finds.
    pub fn pull_requests(&self, workspace_id: &str, query: &str) -> Result<Vec<checkout::PullRequest>, DesktopError> {
        let workspace = self.with_storage(|storage| storage.workspace_get(workspace_id)).map_err(store)?.ok_or_else(|| DesktopError::not_ready("that workspace is gone"))?;
        let program = self.gh().ok_or_else(|| DesktopError::not_ready("GitHub CLI (gh) is not installed"))?;
        checkout::pull_requests(&program, Path::new(&workspace.canonical_root), query)
    }

    /// Reviews that were running when ThingMaker closed; called once at start.
    pub fn mark_interrupted(&self) {
        let running = self.with_storage(|storage| storage.ultra_review_running()).unwrap_or_default();
        for mut review in running {
            review.status = Status::Interrupted;
            review.updated_at = now_unix_ms();
            let _ = self.with_storage(|storage| storage.ultra_review_save(&review));
        }
    }

    /// Checks the change out, seats the panel and starts the review.
    pub async fn start(&self, request: StartRequest) -> Result<Review, DesktopError> {
        let reviewers = seat(&request.seats).map_err(DesktopError::not_ready)?;
        if self.list(&request.host)?.iter().any(|review| review.is_live()) {
            return Err(DesktopError::conflict("A review is already running in this session."));
        }
        let workspace = self.with_storage(|storage| storage.workspace_get(&request.workspace_id)).map_err(store)?.ok_or_else(|| DesktopError::not_ready("that workspace is gone"))?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let service = self.clone();
        let (kind, number, review_id) = (request.kind, request.number, id.clone());
        let root = PathBuf::from(&workspace.canonical_root);
        let (workspace_id, hash) = (workspace.id.clone(), workspace.workspace_hash.clone());
        let checkout: Checkout = tokio::task::spawn_blocking(move || {
            let data_dir = service.inner.data_dir.clone();
            service.with_storage(|storage| match kind {
                TargetKind::Pr => {
                    let number = number.ok_or_else(|| DesktopError::not_ready("pick a pull request"))?;
                    let program = service.gh().ok_or_else(|| DesktopError::not_ready("GitHub CLI (gh) is not installed: install it, or review this branch's changes instead"))?;
                    checkout::pull_request_checkout(storage, &data_dir, &workspace_id, &hash, &root, &program, number, &review_id)
                }
                TargetKind::Branch => checkout::branch_checkout(storage, &data_dir, &workspace_id, &hash, &root, &review_id),
            })
        })
        .await
        .map_err(|error| DesktopError::io(error.to_string()))??;
        let now = now_unix_ms();
        let review = Review {
            id,
            workspace_id: request.workspace_id,
            host: request.host,
            target: checkout.target,
            config: Config { rounds: request.config.rounds.clamp(1, 3), ..request.config },
            reviewers,
            findings: Vec::new(),
            verdict: None,
            phase: Phase::Blind,
            round: 1,
            status: Status::Running,
            error: None,
            root: checkout.root.to_string_lossy().into_owned(),
            worktree_id: Some(checkout.worktree_id),
            checkout_workspace: Some(checkout.checkout_workspace),
            log: Vec::new(),
            created_at: now,
            updated_at: now,
            finished_at: None,
        };
        Ok(self.begin(review))
    }

    /// Starts a review whose checkout is ready: saves it and runs it in the
    /// background. `start` checks the change out first; tests call this.
    pub fn begin(&self, mut review: Review) -> Review {
        self.save(&mut review);
        self.spawn(review.clone());
        review
    }

    /// Picks an interrupted review up where it stopped.
    pub fn resume(&self, id: &str) -> Result<Review, DesktopError> {
        let mut review = self.get(id)?;
        if review.status != Status::Interrupted {
            return Err(DesktopError::conflict("Only an interrupted review can be resumed."));
        }
        if !Path::new(&review.root).exists() {
            return Err(DesktopError::not_ready("The review's checkout is gone; start a new review instead."));
        }
        review.status = Status::Running;
        self.save(&mut review);
        self.spawn(review.clone());
        Ok(review)
    }

    pub async fn cancel(&self, id: &str) -> Result<(), DesktopError> {
        let flag = self.inner.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(id).cloned();
        match flag {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                self.inner.runner.close(id).await;
            }
            None => {
                let mut review = self.get(id)?;
                if matches!(review.status, Status::Interrupted | Status::Running) {
                    review.status = Status::Cancelled;
                    review.finished_at = Some(now_unix_ms());
                    self.cleanup(&review);
                    self.save(&mut review);
                }
            }
        }
        Ok(())
    }

    /// The user's word on a finding, for the next turns and the Chair.
    pub fn note(&self, id: &str, finding: &str, text: &str) -> Result<Review, DesktopError> {
        if text.trim().is_empty() {
            return Err(DesktopError::not_ready("The note is empty."));
        }
        self.update(id, |review| {
            if !debate::add_note(review, finding, &text.chars().take(2_000).collect::<String>(), now_unix_ms()) {
                return Err(DesktopError::not_ready("No such finding."));
            }
            debate::settle(review);
            Ok(())
        })
    }

    /// The user's call on a finding the panel split over.
    pub fn decide(&self, id: &str, finding: &str, decision: Option<Decision>) -> Result<Review, DesktopError> {
        self.update(id, |review| {
            let item = review.verdict.as_mut().and_then(|verdict| verdict.items.iter_mut().find(|item| item.finding == finding)).ok_or_else(|| DesktopError::not_ready("No such finding in the verdict."))?;
            item.decision = decision;
            Ok(())
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), DesktopError> {
        let review = self.get(id)?;
        if review.is_live() {
            return Err(DesktopError::conflict("Stop the review before deleting it."));
        }
        self.cleanup(&review);
        self.with_storage(|storage| storage.ultra_review_delete(id)).map_err(store)
    }

    /// Changes a stored review. A running review's own copy is changed too,
    /// through the same storage, at its next save (see `drive`).
    fn update(&self, id: &str, change: impl FnOnce(&mut Review) -> Result<(), DesktopError>) -> Result<Review, DesktopError> {
        let mut review = self.get(id)?;
        change(&mut review)?;
        self.save(&mut review);
        Ok(review)
    }

    fn cleanup(&self, review: &Review) {
        let repository = self.with_storage(|storage| storage.workspace_get(&review.workspace_id)).ok().flatten().map(|workspace| PathBuf::from(workspace.canonical_root));
        if let Some(repository) = repository {
            self.with_storage(|storage| checkout::remove(storage, &repository, Path::new(&review.root), review.worktree_id.as_deref(), review.checkout_workspace.as_deref()));
        }
    }

    fn spawn(&self, review: Review) {
        let flag = Arc::new(AtomicBool::new(false));
        self.inner.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(review.id.clone(), flag.clone());
        let service = self.clone();
        tokio::spawn(async move {
            let id = review.id.clone();
            service.drive(review, flag).await;
            service.inner.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id);
        });
    }

    /// Notes the user added while turns ran are kept: the stored copy's
    /// threads are merged into the running one before each save.
    fn merge_notes(&self, review: &mut Review) {
        let Ok(Some(stored)) = self.with_storage(|storage| storage.ultra_review_get(&review.id)) else { return };
        for stored_finding in stored.findings {
            let Some(finding) = review.findings.iter_mut().find(|finding| finding.id == stored_finding.id) else { continue };
            for entry in stored_finding.thread.into_iter().filter(|entry| entry.by == super::model::USER) {
                if !finding.thread.contains(&entry) {
                    finding.thread.push(entry);
                }
            }
            finding.thread.sort_by_key(|entry| entry.at);
        }
        debate::settle(review);
    }

    fn commit(&self, review: &mut Review) {
        self.merge_notes(review);
        self.save(review);
    }

    async fn drive(&self, mut review: Review, stop: Arc<AtomicBool>) {
        if let Err(error) = self.inner.runner.open(&review) {
            review.status = Status::Failed;
            review.error = Some(error);
            self.finish(&mut review).await;
            return;
        }
        loop {
            if stop.load(Ordering::SeqCst) {
                review.status = Status::Cancelled;
                break;
            }
            match review.phase {
                Phase::Blind => {
                    self.step(&mut review, &stop, "blind", |review, reviewer| Some((Ask::Blind, prompt::blind(review, reviewer)))).await;
                    review.phase = if review.findings.is_empty() || review.present().count() < 2 { Phase::Verdict } else { Phase::Cross };
                }
                Phase::Cross => {
                    let round = review.round;
                    self.step(&mut review, &stop, &format!("cross:{round}"), |review, reviewer| {
                        let asked = debate::asked_in_cross(review, &reviewer.id, round);
                        (!asked.is_empty()).then(|| (Ask::Cross(asked.clone()), prompt::cross(review, reviewer, &asked, round)))
                    })
                    .await;
                    review.phase = if review.config.quick || debate::all_settled(&review) { Phase::Verdict } else { Phase::Rebuttal };
                }
                Phase::Rebuttal => {
                    let round = review.round;
                    self.step(&mut review, &stop, &format!("rebuttal:{round}"), |review, reviewer| {
                        let asked = debate::asked_in_rebuttal(review, &reviewer.id, round);
                        (!asked.is_empty()).then(|| (Ask::Rebuttal(asked.clone()), prompt::rebuttal(review, reviewer, &asked, round)))
                    })
                    .await;
                    if round >= review.config.cycles() || debate::all_settled(&review) {
                        review.phase = Phase::Verdict;
                    } else {
                        review.round = round + 1;
                        review.phase = Phase::Cross;
                    }
                }
                Phase::Verdict => {
                    let known: Vec<String> = review.findings.iter().map(|finding| finding.id.clone()).collect();
                    let chair = review.chair().filter(|chair| !chair.absent).map(|chair| chair.id.clone());
                    if let Some(chair) = chair {
                        self.step(&mut review, &stop, "verdict", |review, reviewer| (reviewer.id == chair).then(|| (Ask::Verdict(known.clone()), prompt::verdict(review, reviewer)))).await;
                    }
                    if review.verdict.is_none() && !stop.load(Ordering::SeqCst) {
                        debate::fallback_verdict(&mut review);
                    }
                    review.phase = Phase::Done;
                }
                Phase::Done => {
                    review.status = Status::Done;
                    break;
                }
            }
            if !stop.load(Ordering::SeqCst) {
                self.commit(&mut review);
            }
        }
        self.finish(&mut review).await;
    }

    /// One phase: a turn for every present reviewer that has something to
    /// answer and has not answered it yet, all at once, each answer recorded
    /// as it comes in.
    async fn step(&self, review: &mut Review, stop: &AtomicBool, key: &str, ask: impl Fn(&Review, &Reviewer) -> Option<(Ask, String)>) {
        let mut turns = tokio::task::JoinSet::new();
        for reviewer in review.present().filter(|reviewer| reviewer.done.as_deref() != Some(key)) {
            let Some((what, task)) = ask(review, reviewer) else { continue };
            let runner = self.inner.runner.clone();
            let (review_id, seat) = (review.id.clone(), reviewer.clone());
            turns.spawn(async move {
                let id = seat.id.clone();
                (id, run_turn(runner, review_id, seat, what, task).await)
            });
        }
        while let Some(joined) = turns.join_next().await {
            let Ok((reviewer, outcome)) = joined else { continue };
            if stop.load(Ordering::SeqCst) {
                continue;
            }
            let now = now_unix_ms();
            let round = review.round;
            match outcome {
                Outcome::Answered { job, answer } => {
                    match answer {
                        Answer::Findings(found) => debate::add_findings(review, &reviewer, found, now),
                        Answer::Replies(replies) => debate::apply_replies(review, &reviewer, replies, round, now),
                        Answer::Rebuttals(rebuttals) => debate::apply_rebuttals(review, &reviewer, rebuttals, round, now),
                        Answer::Verdict(verdict) => debate::apply_verdict(review, verdict),
                    }
                    if let Some(seat) = review.reviewers.iter_mut().find(|seat| seat.id == reviewer) {
                        seat.done = Some(key.to_string());
                        seat.last_job = Some(job);
                    }
                }
                Outcome::Unreadable { job, error } => {
                    review.log.push(format!("Reviewer {reviewer}'s reply in {} could not be read ({error}), so it counts as no answer.", phase_name(key)));
                    if let Some(seat) = review.reviewers.iter_mut().find(|seat| seat.id == reviewer) {
                        seat.done = Some(key.to_string());
                        seat.last_job = Some(job);
                    }
                }
                Outcome::Failed(error) => {
                    review.log.push(format!("Reviewer {reviewer} dropped out in {}: {error}", phase_name(key)));
                    if let Some(seat) = review.reviewers.iter_mut().find(|seat| seat.id == reviewer) {
                        seat.absent = true;
                    }
                    debate::settle(review);
                }
            }
            self.commit(review);
        }
    }

    async fn finish(&self, review: &mut Review) {
        self.inner.runner.close(&review.id).await;
        self.cleanup(review);
        review.finished_at = Some(now_unix_ms());
        self.merge_notes(review);
        self.save(review);
    }
}

fn phase_name(key: &str) -> String {
    match key.split_once(':') {
        Some(("cross", round)) => format!("cross-examination round {round}"),
        Some(("rebuttal", round)) => format!("rebuttal round {round}"),
        _ if key == "blind" => "the blind review".into(),
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ultrareview::model::{Outcome as VerdictOutcome, Standing};

    type Reply = Box<dyn Fn(&Reviewer, &str) -> Result<String, String> + Send + Sync>;

    /// Replies scripted per reviewer, per phase, read off the prompt.
    struct Script {
        log: Mutex<Vec<String>>,
        reply: Reply,
    }

    impl Runner for Script {
        fn open(&self, _review: &Review) -> Result<(), String> {
            Ok(())
        }
        fn turn(&self, _review_id: &str, reviewer: &Reviewer, task: String, _follow_up: Option<String>) -> BoxFuture<Result<Turn, String>> {
            let answer = (self.reply)(reviewer, &task);
            let mut log = self.log.lock().unwrap();
            log.push(format!("{}:{}", reviewer.id, task.lines().find(|line| line.starts_with("Blind") || line.starts_with("Cross") || line.starts_with("Rebuttal") || line.starts_with("You are also the Chair") || line.starts_with("Your last reply")).unwrap_or("?")));
            let job = format!("job-{}", log.len());
            Box::pin(async move { answer.map(|text| Turn { job, text }) })
        }
        fn close(&self, _review_id: &str) -> BoxFuture<()> {
            Box::pin(async {})
        }
    }

    fn block(json: &str) -> String {
        format!("Done.\n```json\n{json}\n```")
    }

    fn service(script: Script) -> (UltraReviewService, Arc<Script>, Arc<Mutex<Vec<Review>>>) {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        let script = Arc::new(script);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let service = UltraReviewService::new(storage, script.clone(), std::env::temp_dir(), None, move |review: &Review| sink.lock().unwrap().push(review.clone()));
        (service, script, seen)
    }

    async fn run(service: &UltraReviewService, review: Review) -> Review {
        let mut review = review;
        service.save(&mut review);
        service.drive(review.clone(), Arc::new(AtomicBool::new(false))).await;
        service.get(&review.id).unwrap()
    }

    #[tokio::test]
    async fn a_full_review_runs_blind_cross_rebuttal_and_the_chairs_verdict() {
        let (service, script, seen) = service(Script {
            log: Mutex::new(Vec::new()),
            reply: Box::new(|reviewer, task| {
                let id = reviewer.id.as_str();
                Ok(if task.contains("Blind review") {
                    match id {
                        "A" => block(r#"{"findings":[]}"#),
                        "B" => block(r#"{"findings":[{"title":"Timer leaks","severity":"high","file":"a.ts","line":4,"claim":"Never cleared.","evidence":"a.ts:4"}]}"#),
                        _ => block(r#"{"findings":[{"title":"Name clash","severity":"medium","claim":"Two plates read the same."}]}"#),
                    }
                } else if task.contains("Cross-examination") {
                    match id {
                        "A" => block(r#"{"replies":[{"finding":"F1","stance":"confirm","argument":"Reproduced."},{"finding":"F2","stance":"challenge","argument":"Keys differ."}]}"#),
                        "B" => block(r#"{"replies":[{"finding":"F2","stance":"challenge","argument":"Only labels."}]}"#),
                        _ => block(r#"{"replies":[{"finding":"F1","stance":"refine","argument":"Real, but medium.","severity":"medium"}]}"#),
                    }
                } else if task.contains("Rebuttal") {
                    block(r#"{"rebuttals":[{"finding":"F2","action":"withdraw"}]}"#)
                } else {
                    block(r#"{"verdict":{"recommendation":"approve_with_fixes","headline":"One fix","summary":"Fix the timer.","findings":[{"finding":"F1","outcome":"confirmed","severity":"medium","summary":"Real.","fix":"clearInterval"}]}}"#)
                })
            }),
        });
        let mut review = debate::tests_support::review(3);
        review.config.rounds = 2;
        let done = run(&service, review).await;
        assert_eq!(done.status, Status::Done);
        assert_eq!(done.phase, Phase::Done);
        assert_eq!(done.findings.len(), 2);
        assert_eq!(done.findings[1].standing, Standing::Withdrawn);
        let verdict = done.verdict.unwrap();
        assert!(verdict.by_chair);
        assert_eq!(verdict.items.iter().map(|item| item.outcome).collect::<Vec<_>>(), [VerdictOutcome::Confirmed, VerdictOutcome::Withdrawn]);
        let log = script.log.lock().unwrap().clone();
        // Only C raised F2, so only C rebuts; the debate settled after one cycle.
        assert!(log.iter().any(|line| line.starts_with("C:Rebuttal")));
        assert!(!log.iter().any(|line| line.starts_with("A:Rebuttal")));
        assert_eq!(log.iter().filter(|line| line.contains("Cross-examination")).count(), 3);
        assert!(log.last().unwrap().starts_with("A:You are also the Chair"));
        assert!(seen.lock().unwrap().len() > 5, "the renderer hears of every step");
    }

    #[tokio::test]
    async fn an_unreadable_reply_is_asked_for_once_more_and_a_failing_reviewer_drops_out() {
        let (service, script, _) = service(Script {
            log: Mutex::new(Vec::new()),
            reply: Box::new(|reviewer, task| match (reviewer.id.as_str(), task) {
                ("B", _) => Err("the provider crashed".into()),
                ("A", task) if task.starts_with("Your last reply") => Ok(block(r#"{"findings":[{"title":"Real","claim":"x"}]}"#)),
                ("A", task) if task.contains("Blind review") => Ok("I think it is fine, no JSON.".into()),
                (_, task) if task.contains("Blind review") => Ok(block(r#"{"findings":[]}"#)),
                (_, task) if task.contains("Cross-examination") => Ok(block(r#"{"replies":[{"finding":"F1","stance":"confirm","argument":"yes"}]}"#)),
                _ => Ok("no verdict".into()),
            }),
        });
        let done = run(&service, debate::tests_support::review(3)).await;
        assert_eq!(done.status, Status::Done);
        assert!(done.reviewers[1].absent);
        assert!(done.log.iter().any(|line| line.contains("Reviewer B dropped out")));
        assert_eq!(done.findings.len(), 1, "A's second reply was read");
        // The Chair (A) never sent a usable verdict: the debate sums itself up.
        let verdict = done.verdict.unwrap();
        assert!(!verdict.by_chair);
        assert_eq!(verdict.items[0].outcome, VerdictOutcome::Confirmed);
        assert!(script.log.lock().unwrap().iter().any(|line| line.starts_with("A:Your last reply")));
    }

    #[tokio::test]
    async fn quick_mode_has_one_cross_examination_and_no_rebuttal() {
        let (service, script, _) = service(Script {
            log: Mutex::new(Vec::new()),
            reply: Box::new(|reviewer, task| {
                Ok(if task.contains("Blind review") {
                    if reviewer.id == "B" { block(r#"{"findings":[{"title":"X","claim":"x"}]}"#) } else { block(r#"{"findings":[]}"#) }
                } else if task.contains("Cross-examination") {
                    block(r#"{"replies":[{"finding":"F1","stance":"challenge","argument":"no"}]}"#)
                } else {
                    block(r#"{"verdict":{"recommendation":"approve","headline":"Fine","summary":"","findings":[{"finding":"F1","outcome":"refuted","summary":"no"}]}}"#)
                })
            }),
        });
        let mut review = debate::tests_support::review(2);
        review.config.quick = true;
        let done = run(&service, review).await;
        let log = script.log.lock().unwrap().clone();
        assert!(!log.iter().any(|line| line.contains("Rebuttal")), "{log:?}");
        assert_eq!(done.verdict.unwrap().items[0].outcome, VerdictOutcome::Refuted);
    }

    #[tokio::test]
    async fn a_clean_change_goes_straight_to_the_verdict() {
        let (service, script, _) = service(Script {
            log: Mutex::new(Vec::new()),
            reply: Box::new(|_, task| Ok(if task.contains("Blind review") { block(r#"{"findings":[]}"#) } else { block(r#"{"verdict":{"recommendation":"approve","headline":"Clean","summary":"Nothing found."}}"#) })),
        });
        let done = run(&service, debate::tests_support::review(2)).await;
        assert_eq!(done.verdict.unwrap().headline, "Clean");
        assert_eq!(script.log.lock().unwrap().len(), 3, "two blind reviews and the verdict");
    }

    #[test]
    fn a_panel_has_two_to_six_seats_and_one_chair() {
        let seat_of = |chair| SeatRequest { provider: Provider::Codex, model: Some("default".into()), effort: None, lens: Lens::Tests, focus: Some("  ".into()), chair };
        assert!(seat(&[seat_of(false)]).is_err());
        let seated = seat(&[seat_of(false), seat_of(true), seat_of(true)]).unwrap();
        assert_eq!(seated.iter().map(|r| (r.id.as_str(), r.chair)).collect::<Vec<_>>(), [("A", false), ("B", true), ("C", false)]);
        assert_eq!(seated[0].model, None);
        assert_eq!(seated[0].focus, None);
        assert!(seat(&[seat_of(false), seat_of(false)]).unwrap()[0].chair, "the first seat chairs by default");
    }
}
