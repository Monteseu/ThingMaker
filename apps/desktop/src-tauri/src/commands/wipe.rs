//! Wipe All: a project's ThingMaker sessions deleted for good from this
//! computer, at the person's request, for privacy.
//!
//! This is the one place the desktop deletes an agent's transcript (ADR-06
//! otherwise keeps them untouched), and it is scoped tightly:
//!
//! - only sessions ThingMaker started or opened in that project (its own
//!   records: desktop, fork and side sessions), never ones the person ran in
//!   a terminal or another app;
//! - Codex threads are deleted by Codex itself (`thread/delete`, once per
//!   account folder), so its own indexes stay consistent; a file it leaves is
//!   removed after;
//! - Claude Code has no such request, so its files for the session are
//!   removed: the transcript, its subagents and tool results, file history,
//!   task list and session environment;
//! - the prompt-history lines of those sessions go too;
//! - every path is resolved first and must be inside the provider's folder.
//!
//! What the providers keep on their side is not touched: that is theirs, and
//! the person is told so before they confirm.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;
use thingmaker_supervisor::{
    DesktopError,
    agents::{Provider, accounts, claude::transcript_usage::slug_for, codex},
    storage::workspaces::SessionOrigin,
};

use super::{CommandResult, storage_error};
use crate::state::AppState;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeReport {
    pub claude_sessions: usize,
    pub codex_sessions: usize,
    /// Files and folders removed from the providers' folders and ThingMaker's.
    pub files_removed: usize,
    pub history_lines_removed: usize,
    /// Codex threads Codex itself confirmed deleting.
    pub codex_deleted_by_codex: usize,
    /// What could not be removed, in words.
    pub problems: Vec<String>,
}

#[tauri::command]
pub async fn workspace_wipe_sessions(workspace_id: String, state: State<'_, AppState>) -> CommandResult<WipeReport> {
    let record = state
        .with_storage(|s| s.workspace_get(&workspace_id))
        .map_err(storage_error)?
        .ok_or_else(|| DesktopError::not_ready("workspace not found"))?;
    let root = PathBuf::from(&record.canonical_root);
    let sessions: Vec<_> = state
        .with_storage(|s| s.session_list(&workspace_id))
        .map_err(storage_error)?
        .into_iter()
        .filter(|session| matches!(session.origin, SessionOrigin::Desktop | SessionOrigin::Side | SessionOrigin::Fork))
        .filter(|session| matches!(session.provider, Provider::Claude | Provider::Codex))
        .collect();

    // Nothing is deleted under a running agent: it would write the file back.
    let ids: HashSet<&str> = sessions.iter().map(|session| session.agent_session_id.as_str()).collect();
    let live_here = state.actor_roots.lock().unwrap_or_else(|p| p.into_inner()).values().any(|path| path == &root);
    let live_ids = state.agent_session_ids.lock().unwrap_or_else(|p| p.into_inner()).values().any(|id| ids.contains(id.as_str()));
    if live_here || live_ids {
        return Err(DesktopError::conflict("Stop this project's open sessions first."));
    }

    let home = state.home.clone().ok_or_else(|| DesktopError::not_ready("HOME is not set"))?;
    let claude_ids: Vec<String> = sessions.iter().filter(|s| s.provider == Provider::Claude).map(|s| s.agent_session_id.clone()).collect();
    let codex_ids: Vec<String> = sessions.iter().filter(|s| s.provider == Provider::Codex).map(|s| s.agent_session_id.clone()).collect();
    let claude_dirs = provider_folders(Provider::Claude, &home, &state.data_dir);
    let codex_homes = provider_folders(Provider::Codex, &home, &state.data_dir);
    let mut report = WipeReport { claude_sessions: claude_ids.len(), codex_sessions: codex_ids.len(), ..Default::default() };

    // Codex first, by its own hand, one account folder at a time.
    if !codex_ids.is_empty() {
        match state.resolve_provider(Provider::Codex) {
            Ok(resolved) => {
                let program = resolved.program.clone();
                let homes = codex_homes.clone();
                let ids = codex_ids.clone();
                let deleted = tauri::async_runtime::spawn_blocking(move || {
                    let mut confirmed: HashSet<String> = HashSet::new();
                    for home in &homes {
                        let calls: Vec<(&str, Value)> = ids.iter().map(|id| ("thread/delete", json!({ "threadId": id }))).collect();
                        // A thread lives in one account's folder; the others
                        // answer that they do not have it.
                        if let Ok(results) = codex::probe::ask(&program, Some(home), &calls, Duration::from_secs(90)) {
                            for (id, result) in ids.iter().zip(results) {
                                if result.is_ok() {
                                    confirmed.insert(id.clone());
                                }
                            }
                        }
                    }
                    confirmed.len()
                })
                .await
                .unwrap_or(0);
                report.codex_deleted_by_codex = deleted;
            }
            Err(_) => report.problems.push("Codex could not be started, so its threads were removed as files.".into()),
        }
    }

    let removed = wipe_files(&root, &claude_dirs, &codex_homes, &claude_ids, &codex_ids);
    report.files_removed += removed.files;
    report.history_lines_removed += removed.history_lines;
    report.problems.extend(removed.problems);

    // ThingMaker's own records, and the prompts its outbox kept.
    let row_ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();
    let payloads = state.with_storage(|s| s.sessions_delete(&workspace_id, &row_ids)).map_err(storage_error)?;
    let outbox = state.data_dir.join("outbox");
    for payload in payloads {
        match remove_inside(&state.data_dir.join(&payload), std::slice::from_ref(&outbox)) {
            Ok(true) => report.files_removed += 1,
            Ok(false) => {}
            Err(problem) => report.problems.push(problem),
        }
    }
    tracing::info!(workspace = %workspace_id, claude = report.claude_sessions, codex = report.codex_sessions, files = report.files_removed, "wiped sessions");
    Ok(report)
}

/// The person's own folder for a provider and each extra account's.
pub(crate) fn provider_folders(provider: Provider, home: &Path, data_dir: &Path) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = accounts::main_folder(provider, home).into_iter().collect();
    let extra = data_dir.join("accounts").join(provider.as_str());
    if let Ok(entries) = std::fs::read_dir(&extra) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && entry.file_name().to_str().is_some_and(accounts::valid_id) {
                folders.push(path);
            }
        }
    }
    folders
}

#[derive(Debug, Default)]
struct Removed {
    files: usize,
    history_lines: usize,
    problems: Vec<String>,
}

/// Removes the sessions' files from the providers' folders. Account folders
/// link into the person's own, so each real path is removed once.
fn wipe_files(root: &Path, claude_dirs: &[PathBuf], codex_homes: &[PathBuf], claude_ids: &[String], codex_ids: &[String]) -> Removed {
    let mut removed = Removed::default();
    let mut done: HashSet<PathBuf> = HashSet::new();

    if !claude_ids.is_empty() {
        let slug = slug_for(root);
        // Account folders link into the person's own: anything inside any of
        // them is fair game, nothing outside all of them is.
        let base = real_folders(claude_dirs);
        for dir in claude_dirs {
            for id in claude_ids {
                let project = dir.join("projects").join(&slug);
                for path in [
                    project.join(format!("{id}.jsonl")),
                    project.join(id),
                    dir.join("file-history").join(id),
                    dir.join("session-env").join(id),
                    dir.join("tasks").join(id),
                ] {
                    remove_once(path, &base, &mut done, &mut removed);
                }
                for entry in std::fs::read_dir(dir.join("todos")).into_iter().flatten().flatten() {
                    if entry.file_name().to_str().is_some_and(|name| name.starts_with(id.as_str())) {
                        remove_once(entry.path(), &base, &mut done, &mut removed);
                    }
                }
            }
            removed.history_lines += drop_history_lines(&dir.join("history.jsonl"), "sessionId", claude_ids, &mut done, &mut removed.problems);
        }
    }

    if !codex_ids.is_empty() {
        let base = real_folders(codex_homes);
        for home in codex_homes {
            let suffixes: Vec<String> = codex_ids.iter().map(|id| format!("-{id}.jsonl")).collect();
            for folder in ["sessions", "archived_sessions"] {
                for path in files_under(&home.join(folder), 4) {
                    if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix.as_str()))) {
                        remove_once(path, &base, &mut done, &mut removed);
                    }
                }
            }
            for id in codex_ids {
                remove_once(home.join("generated_images").join(id), &base, &mut done, &mut removed);
            }
            removed.history_lines += drop_history_lines(&home.join("history.jsonl"), "session_id", codex_ids, &mut done, &mut removed.problems);
        }
    }
    removed
}

fn real_folders(folders: &[PathBuf]) -> Vec<PathBuf> {
    folders.iter().filter_map(|folder| std::fs::canonicalize(folder).ok()).collect()
}

/// Removes a path once: account folders link into the person's own, so the
/// same real file can be reached twice.
fn remove_once(path: PathBuf, inside: &[PathBuf], done: &mut HashSet<PathBuf>, removed: &mut Removed) {
    let Ok(real) = std::fs::canonicalize(&path) else { return };
    if !done.insert(real.clone()) {
        return;
    }
    match remove_inside(&real, inside) {
        Ok(true) => removed.files += 1,
        Ok(false) => {}
        Err(problem) => removed.problems.push(problem),
    }
}

/// Files up to `depth` folders down (Codex keeps rollouts by year/month/day).
fn files_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && depth > 0 {
            out.extend(files_under(&path, depth - 1));
        } else if kind.is_file() {
            out.push(path);
        }
    }
    out
}

/// Removes a file or folder whose real path must lie inside one of `inside`
/// (also resolved), and not be that folder itself. `Ok(false)` when there was
/// nothing there.
fn remove_inside(path: &Path, inside: &[PathBuf]) -> Result<bool, String> {
    let Ok(real) = std::fs::canonicalize(path) else { return Ok(false) };
    let allowed: Vec<PathBuf> = inside.iter().filter_map(|folder| std::fs::canonicalize(folder).ok()).collect();
    if !allowed.iter().any(|folder| real != *folder && real.starts_with(folder)) {
        return Err(format!("{} is outside the provider's folder, so it was left alone", real.display()));
    }
    let result = if real.is_dir() { std::fs::remove_dir_all(&real) } else { std::fs::remove_file(&real) };
    result.map(|()| true).map_err(|error| format!("{}: {error}", real.display()))
}

/// Rewrites a JSON-lines history file without the lines whose `key` is one of
/// `ids`. Returns how many went.
fn drop_history_lines(path: &Path, key: &str, ids: &[String], done: &mut HashSet<PathBuf>, problems: &mut Vec<String>) -> usize {
    let Ok(real) = std::fs::canonicalize(path) else { return 0 };
    if !done.insert(real.clone()) {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(&real) else { return 0 };
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut kept = String::with_capacity(text.len());
    let mut dropped = 0;
    for line in text.lines() {
        let theirs = serde_json::from_str::<Value>(line).ok().and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string));
        if theirs.as_deref().is_some_and(|id| wanted.contains(id)) {
            dropped += 1;
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if dropped == 0 {
        return 0;
    }
    let temp = real.with_extension("jsonl.thingmaker-wipe");
    if let Err(error) = std::fs::write(&temp, kept).and_then(|()| std::fs::rename(&temp, &real)) {
        let _ = std::fs::remove_file(&temp);
        problems.push(format!("{}: {error}", real.display()));
        return 0;
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_the_named_sessions_files_go_and_linked_folders_are_removed_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = Path::new("/Users/me/hunted");
        let slug = slug_for(root);
        let claude = temp.path().join("dot-claude");
        let codex = temp.path().join("dot-codex");
        let keep = "keep-1";
        let gone = "gone-1";
        write(&claude.join(format!("projects/{slug}/{gone}.jsonl")), "{}");
        write(&claude.join(format!("projects/{slug}/{gone}/subagents/a.jsonl")), "{}");
        write(&claude.join(format!("projects/{slug}/{keep}.jsonl")), "{}");
        write(&claude.join(format!("file-history/{gone}/x")), "x");
        write(&claude.join(format!("todos/{gone}-agent-{gone}.json")), "[]");
        write(&claude.join("history.jsonl"), &format!("{{\"sessionId\":\"{gone}\",\"display\":\"secret\"}}\n{{\"sessionId\":\"{keep}\",\"display\":\"fine\"}}\n"));
        // An extra account's folder, linking into the person's own.
        let account = temp.path().join("accounts/claude/acct-1");
        std::fs::create_dir_all(&account).unwrap();
        std::os::unix::fs::symlink(claude.join("projects"), account.join("projects")).unwrap();
        std::os::unix::fs::symlink(claude.join("history.jsonl"), account.join("history.jsonl")).unwrap();
        let thread = "019a-gone";
        write(&codex.join(format!("sessions/2026/11/12/rollout-2026-11-12T10-00-00-{thread}.jsonl")), "{}");
        write(&codex.join("sessions/2026/11/12/rollout-2026-11-12T10-00-00-019a-keep.jsonl"), "{}");
        write(&codex.join(format!("archived_sessions/rollout-x-{thread}.jsonl")), "{}");
        write(&codex.join(format!("generated_images/{thread}/a.png")), "png");

        let removed = wipe_files(root, &[claude.clone(), account.clone()], std::slice::from_ref(&codex), &[gone.to_string()], &[thread.to_string()]);

        assert!(removed.problems.is_empty(), "{:?}", removed.problems);
        assert!(!claude.join(format!("projects/{slug}/{gone}.jsonl")).exists());
        assert!(!claude.join(format!("projects/{slug}/{gone}")).exists());
        assert!(!claude.join(format!("file-history/{gone}")).exists());
        assert!(!claude.join(format!("todos/{gone}-agent-{gone}.json")).exists());
        assert!(claude.join(format!("projects/{slug}/{keep}.jsonl")).exists(), "another session stays");
        assert!(account.join("projects").exists(), "the account's link stays; only what it points at went");
        let history = std::fs::read_to_string(claude.join("history.jsonl")).unwrap();
        assert!(!history.contains("secret") && history.contains("fine"));
        assert_eq!(removed.history_lines, 1, "the shared history file is rewritten once");
        assert!(!codex.join(format!("sessions/2026/11/12/rollout-2026-11-12T10-00-00-{thread}.jsonl")).exists());
        assert!(codex.join("sessions/2026/11/12/rollout-2026-11-12T10-00-00-019a-keep.jsonl").exists());
        assert!(!codex.join(format!("archived_sessions/rollout-x-{thread}.jsonl")).exists());
        assert!(!codex.join(format!("generated_images/{thread}")).exists());
        assert_eq!(removed.files, 7);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_outside_the_providers_folder_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("elsewhere/important.txt");
        write(&outside, "keep");
        let folder = temp.path().join("dot-claude");
        std::fs::create_dir_all(folder.join("file-history")).unwrap();
        // A link inside the folder that points out of it.
        std::os::unix::fs::symlink(temp.path().join("elsewhere"), folder.join("file-history/evil")).unwrap();
        let removed = wipe_files(Path::new("/p"), std::slice::from_ref(&folder), &[], &["evil".to_string()], &[]);
        assert!(outside.exists());
        assert_eq!(removed.files, 0);
        assert_eq!(removed.problems.len(), 1);
    }
}
