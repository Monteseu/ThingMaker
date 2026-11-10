//! The change under review, checked out where the reviewers can read it and
//! run its tests without touching the user's own tree: a detached worktree
//! under ThingMaker's data folder, registered as a workspace for the length of
//! the review and forgotten afterwards.
//!
//! A pull request is fetched from `origin` by its `pull/N/head` ref and
//! measured from its merge base with the PR's base branch; the current branch
//! is measured from its merge base with the default branch, and its
//! uncommitted changes are carried into the checkout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{Target, TargetKind};
use crate::{
    error::DesktopError,
    review::{self, git},
    storage::Storage,
    workspace::WorkspaceIdentity,
};

/// `gh` on PATH or where its installers put it: an app opened from the Dock
/// has no shell PATH.
pub fn find_gh(home: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "gh.exe" } else { "gh" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = std::env::split_paths(&path).map(|dir| dir.join(name)).collect();
    candidates.extend([PathBuf::from("/opt/homebrew/bin").join(name), PathBuf::from("/usr/local/bin").join(name)]);
    if let Some(home) = home {
        candidates.push(home.join(".local/bin").join(name));
    }
    if cfg!(windows) {
        candidates.push(PathBuf::from(r"C:\Program Files\GitHub CLI").join(name));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn gh(program: &Path, root: &Path, args: &[&str], input: Option<&str>) -> Result<String, DesktopError> {
    use std::io::Write;
    let mut command = crate::platform::command(program);
    command
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|error| DesktopError::io(format!("could not run gh: {error}")))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).map_err(|error| DesktopError::io(error.to_string()))?;
    }
    let output = child.wait_with_output().map_err(|error| DesktopError::io(error.to_string()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DesktopError::io(if message.is_empty() { format!("gh {} failed", args.first().copied().unwrap_or("")) } else { message }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// An open pull request, for the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub author: Option<Author>,
    /// OPEN, CLOSED or MERGED; a pasted link may name a pull request that is no longer open.
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub login: String,
}

const PR_FIELDS: &str = "number,title,headRefName,baseRefName,url,additions,deletions,changedFiles,isDraft,author,state";

/// What the search box holds: a pull request's number or link, or words to search for.
#[derive(Debug, PartialEq, Eq)]
pub enum PrQuery<'a> {
    All,
    /// A number, and the repository a pasted link names (`host/owner/repo`, lowercased).
    Number(u64, Option<String>),
    Text(&'a str),
}

pub fn pr_query(query: &str) -> PrQuery<'_> {
    let query = query.trim();
    if query.is_empty() {
        return PrQuery::All;
    }
    if let Ok(number) = query.trim_start_matches('#').parse::<u64>() {
        return PrQuery::Number(number, None);
    }
    if let Some((repo, rest)) = query.split_once("/pull/") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u64>() {
            return PrQuery::Number(number, Some(repository(repo)));
        }
    }
    PrQuery::Text(query)
}

/// `https://github.com/Owner/Repo` → `github.com/owner/repo`, so links compare whatever their scheme or case.
fn repository(prefix: &str) -> String {
    let prefix = prefix.trim().trim_end_matches('/');
    let prefix = prefix.split_once("://").map_or(prefix, |(_, rest)| rest);
    prefix.trim_start_matches("www.").to_lowercase()
}

/// The open pull requests, or the ones a search finds: by number or link (open
/// or not), or by words in the title, branch or author.
pub fn pull_requests(program: &Path, root: &Path, query: &str) -> Result<Vec<PullRequest>, DesktopError> {
    let output = match pr_query(query) {
        PrQuery::All => gh(program, root, &["pr", "list", "--state", "open", "--limit", "50", "--json", PR_FIELDS], None)?,
        PrQuery::Text(text) => gh(program, root, &["pr", "list", "--state", "open", "--limit", "50", "--search", text, "--json", PR_FIELDS], None)?,
        PrQuery::Number(number, repo) => {
            let found = pull_request(program, root, number)?;
            if let Some(repo) = repo {
                let own = found.url.split_once("/pull/").map(|(prefix, _)| repository(prefix)).unwrap_or_default();
                if !own.is_empty() && own != repo {
                    return Err(DesktopError::unsupported(format!("that link is to a pull request in {repo}, but this workspace is {own}")));
                }
            }
            return Ok(vec![found]);
        }
    };
    serde_json::from_str(&output).map_err(|error| DesktopError::io(format!("gh answered something unexpected: {error}")))
}

pub fn pull_request(program: &Path, root: &Path, number: u64) -> Result<PullRequest, DesktopError> {
    let number = number.to_string();
    let output = gh(program, root, &["pr", "view", &number, "--json", PR_FIELDS], None)?;
    serde_json::from_str(&output).map_err(|error| DesktopError::io(format!("gh answered something unexpected: {error}")))
}

/// Posts the review on the pull request, as a comment from the user's own
/// `gh` account.
pub fn post_review(program: &Path, root: &Path, number: u64, body: &str) -> Result<(), DesktopError> {
    let number = number.to_string();
    gh(program, root, &["pr", "review", &number, "--comment", "--body-file", "-"], Some(body)).map(|_| ())
}

fn trimmed(text: String) -> String {
    text.trim().to_string()
}

/// The branch the repository's default points at (`origin/HEAD`), else
/// `main` or `master`, whichever exists.
fn default_branch(root: &Path) -> String {
    if let Ok(head) = git::run(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]) {
        let head = head.trim();
        if let Some(name) = head.strip_prefix("origin/").filter(|name| !name.is_empty()) {
            return name.to_string();
        }
    }
    for candidate in ["main", "master"] {
        if git::run(root, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return candidate.to_string();
        }
    }
    "main".into()
}

/// Files changed and lines added and removed between `base` and the working
/// tree of `root`.
fn stats(root: &Path, base: &str) -> (u32, u32, u32) {
    let Ok(output) = git::run(root, &["diff", "--numstat", base]) else { return (0, 0, 0) };
    output.lines().fold((0, 0, 0), |(files, added, removed), line| {
        let mut parts = line.split('\t');
        let add = parts.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        let del = parts.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        (files + 1, added + add, removed + del)
    })
}

fn last_subject(root: &Path) -> String {
    git::run(root, &["log", "-1", "--format=%s"]).map(trimmed).unwrap_or_default()
}

/// What a fresh checkout is.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub target: Target,
    pub root: PathBuf,
    pub worktree_id: String,
    pub checkout_workspace: String,
}

fn checkout_path(data_dir: &Path, workspace_hash: &str, review_id: &str) -> PathBuf {
    data_dir.join("worktrees").join(workspace_hash).join(format!("ultra-review-{review_id}"))
}

fn add_detached(root: &Path, path: &Path, rev: &str) -> Result<(), DesktopError> {
    if path.exists() {
        return Err(DesktopError::conflict(format!("{} already exists", path.display())));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| DesktopError::io(error.to_string()))?;
    }
    let path_text = path.to_string_lossy().into_owned();
    // Reviewers read code: large files kept in Git LFS stay as their small
    // pointer files instead of being downloaded into every review's copy.
    let added = git::run_with_env(root, &["worktree", "add", "--detach", &path_text, rev], &[("GIT_LFS_SKIP_SMUDGE", "1")]);
    if let Err(error) = added {
        // Nothing half-made is left behind for the next try.
        let _ = git::run(root, &["worktree", "remove", "--force", &path_text]);
        let _ = std::fs::remove_dir_all(path);
        let _ = git::run(root, &["worktree", "prune"]);
        if error.message.contains("git-lfs") && error.message.contains("not found") {
            return Err(DesktopError::not_ready("this repository keeps files in Git LFS, and git-lfs is not installed. Install it (brew install git-lfs) and try again"));
        }
        return Err(error);
    }
    Ok(())
}

/// Registers the checkout so ThingMaker can open the reviewers' sessions in it.
fn register(storage: &Storage, workspace_id: &str, path: &Path, label: &str, base: &str) -> Result<(String, String), DesktopError> {
    let store = |error: crate::storage::StorageError| DesktopError::io(error.to_string());
    let worktree = storage.worktree_insert(workspace_id, &path.to_string_lossy(), label, base).map_err(store)?;
    let identity = WorkspaceIdentity::resolve(path)?;
    let record = storage.workspace_upsert(&identity.canonical_root.to_string_lossy(), &identity.display_path, &identity.workspace_hash).map_err(store)?;
    Ok((worktree.id, record.id))
}

/// Checks out pull request `number` of the repository at `root`.
#[allow(clippy::too_many_arguments)]
pub fn pull_request_checkout(storage: &Storage, data_dir: &Path, workspace_id: &str, workspace_hash: &str, root: &Path, gh_program: &Path, number: u64, review_id: &str) -> Result<Checkout, DesktopError> {
    let pr = pull_request(gh_program, root, number)?;
    let head_ref = format!("refs/thingmaker/ultra-review/{review_id}");
    git::run(root, &["fetch", "--no-tags", "--quiet", "origin", &format!("+pull/{number}/head:{head_ref}")])
        .map_err(|error| DesktopError::io(format!("could not fetch pull request #{number} from origin: {error}")))?;
    let base_branch = format!("refs/remotes/origin/{}", pr.base_ref_name);
    git::run(root, &["fetch", "--no-tags", "--quiet", "origin", &format!("+refs/heads/{}:{base_branch}", pr.base_ref_name)])
        .map_err(|error| DesktopError::io(format!("could not fetch {} from origin: {error}", pr.base_ref_name)))?;
    let head_sha = git::run(root, &["rev-parse", &head_ref]).map(trimmed)?;
    let base_sha = git::run(root, &["merge-base", &base_branch, &head_ref]).map(trimmed)?;
    let path = checkout_path(data_dir, workspace_hash, review_id);
    add_detached(root, &path, &head_sha)?;
    // The fetched ref only had to live until the checkout existed.
    let _ = git::run(root, &["update-ref", "-d", &head_ref]);
    let (files, additions, deletions) = stats(&path, &base_sha);
    let (worktree_id, checkout_workspace) = register(storage, workspace_id, &path, &format!("PR #{number} (Ultra-Review)"), &base_sha)?;
    Ok(Checkout {
        target: Target { kind: TargetKind::Pr, number: Some(number), title: pr.title, url: Some(pr.url).filter(|url| !url.is_empty()), base_ref: pr.base_ref_name, head_ref: pr.head_ref_name, base_sha, head_sha, files, additions, deletions },
        root: path,
        worktree_id,
        checkout_workspace,
    })
}

/// Checks out the workspace's current branch with its uncommitted changes.
pub fn branch_checkout(storage: &Storage, data_dir: &Path, workspace_id: &str, workspace_hash: &str, root: &Path, review_id: &str) -> Result<Checkout, DesktopError> {
    if !review::is_git_repository(root) {
        return Err(DesktopError::not_ready("this workspace is not a Git repository"));
    }
    let head_sha = git::run(root, &["rev-parse", "HEAD"]).map(trimmed).map_err(|_| DesktopError::not_ready("this repository has no commits yet"))?;
    let head_ref = git::run(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(trimmed).unwrap_or_else(|_| "HEAD".into());
    let base_ref = default_branch(root);
    // On the default branch itself, the change is what is not committed yet.
    let base_sha = if head_ref == base_ref { head_sha.clone() } else { git::run(root, &["merge-base", &base_ref, "HEAD"]).map(trimmed).unwrap_or_else(|_| head_sha.clone()) };
    let (patch, untracked) = review::uncommitted_snapshot(root)?;
    let path = checkout_path(data_dir, workspace_hash, review_id);
    add_detached(root, &path, &head_sha)?;
    review::transfer_uncommitted(root, &path, &patch, &untracked)?;
    // New files show in `git diff <base>` once Git knows they are meant to be there.
    let _ = git::run(&path, &["add", "--intent-to-add", "--all"]);
    let (files, additions, deletions) = stats(&path, &base_sha);
    if files == 0 {
        let _ = git::run(root, &["worktree", "remove", "--force", &path.to_string_lossy()]);
        return Err(DesktopError::not_ready(format!("{head_ref} has no changes against {base_ref} to review")));
    }
    let title = if head_ref == base_ref { "Uncommitted changes".to_string() } else { last_subject(&path) };
    let (worktree_id, checkout_workspace) = register(storage, workspace_id, &path, &format!("{head_ref} (Ultra-Review)"), &base_sha)?;
    Ok(Checkout {
        target: Target { kind: TargetKind::Branch, number: None, title, url: None, base_ref, head_ref, base_sha, head_sha, files, additions, deletions },
        root: path,
        worktree_id,
        checkout_workspace,
    })
}

/// Removes a review's checkout and forgets the workspace it was registered
/// as. The review itself, its findings and the reviewers' transcripts stay.
pub fn remove(storage: &Storage, repository_root: &Path, checkout: &Path, worktree_id: Option<&str>, checkout_workspace: Option<&str>) {
    if checkout.exists()
        && let Err(error) = git::run(repository_root, &["worktree", "remove", "--force", &checkout.to_string_lossy()])
    {
        tracing::warn!(%error, path = %checkout.display(), "could not remove an Ultra-Review checkout");
        let _ = std::fs::remove_dir_all(checkout);
        let _ = git::run(repository_root, &["worktree", "prune"]);
    }
    if let Some(id) = worktree_id {
        let _ = storage.worktree_mark_removed(id);
    }
    if let Some(id) = checkout_workspace {
        let _ = storage.workspace_delete(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn a_review_copy_of_a_git_lfs_repository_keeps_pointer_files() {
        if !Command::new("git").args(["lfs", "version"]).output().map(|o| o.status.success()).unwrap_or(false) {
            return; // git-lfs is not installed here.
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        assert!(sh(&root, &["init", "-q", "-b", "main"]));
        assert!(sh(&root, &["config", "user.email", "t@example.com"]) && sh(&root, &["config", "user.name", "T"]));
        assert!(sh(&root, &["lfs", "install", "--local"]));
        assert!(sh(&root, &["lfs", "track", "*.bin"]));
        std::fs::write(root.join("art.bin"), vec![7u8; 4096]).unwrap();
        std::fs::write(root.join("code.txt"), "hello\n").unwrap();
        assert!(sh(&root, &["add", "."]) && sh(&root, &["commit", "-q", "-m", "init"]));
        // The large file is not here and there is no server to fetch it from:
        // a normal checkout fails, as it did on the user's machine.
        std::fs::remove_dir_all(root.join(".git/lfs/objects")).unwrap();
        let copy = dir.path().join("copy");
        add_detached(&root, &copy, "HEAD").unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("code.txt")).unwrap(), "hello\n");
        assert!(std::fs::read_to_string(copy.join("art.bin")).unwrap().starts_with("version https://git-lfs.github.com/spec/v1"));
    }

    #[test]
    fn the_search_box_reads_numbers_links_and_words() {
        assert_eq!(pr_query("  "), PrQuery::All);
        assert_eq!(pr_query("#1560"), PrQuery::Number(1560, None));
        assert_eq!(pr_query("1560"), PrQuery::Number(1560, None));
        assert_eq!(pr_query("https://github.com/Madbox/Game/pull/1560/files?w=1"), PrQuery::Number(1560, Some("github.com/madbox/game".into())));
        assert_eq!(pr_query("http://www.github.com/madbox/game/pull/7"), PrQuery::Number(7, Some("github.com/madbox/game".into())));
        assert_eq!(pr_query("black market"), PrQuery::Text("black market"));
    }

    fn sh(root: &Path, args: &[&str]) -> bool {
        Command::new("git").arg("-C").arg(root).args(args).output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[test]
    fn a_branch_is_checked_out_with_its_uncommitted_changes_and_removed_after() {
        let repo = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = repo.path();
        if !sh(root, &["init", "-q", "-b", "main"]) {
            eprintln!("skipping: git unavailable");
            return;
        }
        sh(root, &["config", "user.email", "t@example.com"]);
        sh(root, &["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "first"]);
        sh(root, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        sh(root, &["commit", "-q", "-am", "Add two"]);
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();

        let storage = Storage::open_in_memory().unwrap();
        let canonical = std::fs::canonicalize(root).unwrap();
        let workspace = storage.workspace_upsert(&canonical.to_string_lossy(), &canonical.to_string_lossy(), "hash").unwrap();
        let checkout = branch_checkout(&storage, data.path(), &workspace.id, "hash", &canonical, "r1").unwrap();
        assert_eq!((checkout.target.kind, checkout.target.head_ref.as_str(), checkout.target.base_ref.as_str()), (TargetKind::Branch, "feature", "main"));
        assert_eq!(checkout.target.title, "Add two");
        assert_eq!((checkout.target.files, checkout.target.additions), (2, 2), "the commit and the untracked file");
        assert!(checkout.root.join("new.txt").is_file());
        assert!(storage.workspace_get(&checkout.checkout_workspace).unwrap().is_some());

        remove(&storage, &canonical, &checkout.root, Some(&checkout.worktree_id), Some(&checkout.checkout_workspace));
        assert!(!checkout.root.exists());
        assert!(storage.workspace_get(&checkout.checkout_workspace).unwrap().is_none());
        assert!(root.join("new.txt").is_file(), "the user's own tree is untouched");
    }

    #[test]
    fn nothing_to_review_is_said_plainly() {
        let repo = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = repo.path();
        if !sh(root, &["init", "-q", "-b", "main"]) {
            return;
        }
        sh(root, &["config", "user.email", "t@example.com"]);
        sh(root, &["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "first"]);
        let storage = Storage::open_in_memory().unwrap();
        let canonical = std::fs::canonicalize(root).unwrap();
        let error = branch_checkout(&storage, data.path(), "w", "hash", &canonical, "r2").unwrap_err();
        assert!(error.to_string().contains("no changes"), "{error}");
    }
}
