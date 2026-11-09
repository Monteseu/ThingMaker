//! Ultra-Review end to end through delegation: reviewers are real worker
//! sessions (the Claude mock playing a reviewer), pinned by name, and the
//! review runs from the blind review to the Chair's verdict.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use thingmaker_supervisor::{
    DesktopError,
    acp::LaunchTarget,
    agents::{PermissionStance, Provider, claude::launch::ClaudeLaunchOptions},
    delegation::{Delegation, WorkerLauncher, WorkerSpec, jobs::BoxFuture},
    storage::Storage,
    supervisor::{AgentLaunch, SessionActor, SessionActorConfig},
    ultrareview::{
        DelegationRunner, UltraReviewService,
        model::{Config, Lens, Outcome, Phase, Review, Reviewer, Status, Target, TargetKind},
    },
};

fn python() -> Option<PathBuf> {
    ["python3", "python"].into_iter().map(PathBuf::from).find(|candidate| std::process::Command::new(candidate).arg("--version").output().is_ok_and(|output| output.status.success()))
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/test-fixtures/acp").join(name).canonicalize().unwrap().to_string_lossy().into_owned()
}

struct ReviewerLauncher {
    python: PathBuf,
    launched: Mutex<Vec<String>>,
}

impl WorkerLauncher for ReviewerLauncher {
    fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
        self.launched.lock().unwrap().push(spec.slot.name.clone());
        let mut options = ClaudeLaunchOptions::new(&spec.root, self.python.clone(), vec![fixture("mock-claude-acp.py"), "--review".into()]);
        options.permission_mode = PermissionStance::AcceptEdits;
        let target = LaunchTarget::executable(self.python.clone());
        Box::pin(async move {
            let mut config = SessionActorConfig::new(target, AgentLaunch::Claude(options));
            config.timeouts.turn = Duration::from_secs(10);
            SessionActor::open(config).await
        })
    }

    fn released(&self, _worker_handle: &str) {}
}

fn seat(id: &str, chair: bool) -> Reviewer {
    Reviewer { id: id.into(), provider: Provider::Claude, model: None, effort: None, lens: Lens::General, focus: None, chair, absent: false, done: None, last_job: None }
}

#[tokio::test]
async fn a_review_runs_through_real_worker_sessions_to_the_chairs_verdict() {
    let Some(python) = python() else {
        eprintln!("skipping: python unavailable");
        return;
    };
    let checkout = tempfile::tempdir().unwrap();
    std::fs::write(checkout.path().join("a.txt"), "two\n").unwrap();
    let launcher = Arc::new(ReviewerLauncher { python, launched: Mutex::new(Vec::new()) });
    let delegation = Delegation::new(launcher.clone(), |_| {});
    let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
    let heard = Arc::new(Mutex::new(0usize));
    let counter = heard.clone();
    let service = UltraReviewService::new(storage, Arc::new(DelegationRunner { delegation: delegation.clone() }), std::env::temp_dir(), None, move |_| *counter.lock().unwrap() += 1);
    let review = Review {
        id: "itest".into(),
        workspace_id: "w".into(),
        host: "agent-1".into(),
        target: Target { kind: TargetKind::Branch, number: None, title: "Say two".into(), url: None, base_ref: "main".into(), head_ref: "feature".into(), base_sha: "aaaa".into(), head_sha: "bbbb".into(), files: 1, additions: 1, deletions: 1 },
        config: Config { quick: false, rounds: 2, require_evidence: true, allow_tests: false },
        reviewers: vec![seat("A", true), seat("B", false), seat("C", false)],
        findings: Vec::new(),
        verdict: None,
        phase: Phase::Blind,
        round: 1,
        status: Status::Running,
        error: None,
        root: checkout.path().to_string_lossy().into_owned(),
        worktree_id: None,
        checkout_workspace: None,
        log: Vec::new(),
        created_at: 1,
        updated_at: 1,
        finished_at: None,
    };
    service.begin(review);

    let mut done = None;
    for _ in 0..300 {
        let current = service.get("itest").unwrap();
        if current.status != Status::Running {
            done = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let done = done.expect("the review finished");
    assert_eq!(done.status, Status::Done, "{:?}", done.log);
    assert_eq!(done.findings.len(), 1);
    assert_eq!(done.findings[0].author, "A");
    // B and C confirmed it in the first cross-examination: nothing left to argue.
    assert!(done.findings[0].settled);
    let verdict = done.verdict.expect("a verdict");
    assert!(verdict.by_chair);
    assert_eq!(verdict.items[0].outcome, Outcome::Confirmed);
    // Every reviewer ran as its own pinned worker, and they were let go.
    let launched = launcher.launched.lock().unwrap().clone();
    for name in ["Reviewer A", "Reviewer B", "Reviewer C"] {
        assert!(launched.iter().any(|launched| launched == name), "{name} in {launched:?}");
    }
    assert!(delegation.jobs("ultra-review:itest").iter().all(|job| job.status.is_finished()));
    assert!(*heard.lock().unwrap() > 4, "the renderer heard each step");
}
