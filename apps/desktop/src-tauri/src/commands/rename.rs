//! Renaming a project's folder from the desktop, so the person does not lose
//! the project's sessions by renaming it behind ThingMaker's back.
//!
//! The folder is renamed first; if that fails nothing has changed. Then the
//! desktop's records follow it (rolled back with the folder if they cannot),
//! and after that, best effort, everything else that names the folder: Claude
//! Code's transcript folder and per-project settings, Codex's per-project
//! settings, and git's worktree links. What could not be done is reported in
//! words. Documentation inside the project is left to an agent the person
//! may start afterwards.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::Serialize;
use tauri::State;
use thingmaker_supervisor::{
    DesktopError,
    agents::{Provider, claude::transcript_usage::slug_for},
    storage::workspaces::WorkspaceRecord,
    workspace::{canonical_root, rename},
};

use super::{CommandResult, storage_error, wipe::provider_folders};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameReport {
    pub workspace: WorkspaceRecord,
    /// The folder before and after, as people write paths.
    pub old_root: String,
    pub new_root: String,
    pub claude_transcripts_moved: usize,
    pub config_entries_updated: usize,
    pub git_worktrees_repaired: bool,
    /// What could not be updated, in words.
    pub problems: Vec<String>,
}

#[tauri::command]
pub async fn workspace_rename_folder(workspace_id: String, new_name: String, state: State<'_, AppState>) -> CommandResult<RenameReport> {
    let record = state
        .with_storage(|s| s.workspace_get(&workspace_id))
        .map_err(storage_error)?
        .ok_or_else(|| DesktopError::not_ready("workspace not found"))?;
    if let Some(reason) = rename::invalid_folder_name(&new_name) {
        return Err(DesktopError::unsupported(reason));
    }
    let old_root = PathBuf::from(&record.canonical_root);
    if old_root.starts_with(state.data_dir.join("worktrees")) {
        return Err(DesktopError::unsupported("This folder is a worktree ThingMaker manages; rename the project it belongs to instead."));
    }
    let parent = old_root.parent().ok_or_else(|| DesktopError::unsupported("A drive's root folder cannot be renamed."))?;
    let target = parent.join(&new_name);
    let old_name = old_root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if old_name == new_name {
        return Err(DesktopError::unsupported("That is already the folder's name."));
    }
    // A change of case only is the same folder on Windows and macOS.
    let case_only = old_name.eq_ignore_ascii_case(&new_name);
    if target.exists() && !case_only {
        return Err(DesktopError::conflict(format!("A folder named {new_name} already exists next to it.")));
    }
    if !state.actors_under(&old_root).is_empty() {
        return Err(DesktopError::conflict("Stop this project's open sessions first: their agents are working in the folder."));
    }

    // Workspaces inside this one move with it: their roots before and after.
    let nested: Vec<(String, String)> = state
        .with_storage(|s| s.workspace_list())
        .map_err(storage_error)?
        .into_iter()
        .filter(|workspace| Path::new(&workspace.canonical_root).starts_with(&old_root))
        .filter_map(|workspace| rename::renamed_key(&workspace.canonical_root, &record.canonical_root, &new_name).map(|new| (workspace.canonical_root, new)))
        .collect();

    std::fs::rename(&old_root, &target).map_err(|error| {
        let terminals = state.terminal_workspaces.lock().unwrap_or_else(|p| p.into_inner()).values().any(|id| id == &workspace_id);
        let hint = if terminals {
            " Close this project's terminals, and any editor or Explorer window showing the folder, then try again."
        } else {
            " Close any editor, terminal or Explorer window using the folder, then try again."
        };
        DesktopError::io(format!("Could not rename {}: {error}.{hint}", rename::ordinary(&record.canonical_root)))
    })?;

    let new_root = canonical_root(&target).unwrap_or_else(|_| target.clone());
    let new_root_text = new_root.to_string_lossy().into_owned();
    let new_display = rename::renamed_key(&record.display_path, &record.display_path, &new_name).unwrap_or_else(|| rename::ordinary(&new_root_text));
    let workspace = match state.with_storage(|s| s.workspace_relocate(&workspace_id, &record.canonical_root, &new_root_text, &new_display)) {
        Ok(workspace) => workspace,
        Err(error) => {
            // The records could not follow: put the folder back.
            let _ = std::fs::rename(&target, &old_root);
            return Err(storage_error(error));
        }
    };

    let mut report = RenameReport {
        workspace,
        old_root: rename::ordinary(&record.canonical_root),
        new_root: rename::ordinary(&new_root_text),
        claude_transcripts_moved: 0,
        config_entries_updated: 0,
        git_worktrees_repaired: false,
        problems: Vec::new(),
    };

    if let Some(home) = state.home.clone() {
        // Account folders link into the person's own: each real folder once.
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let claude_dirs = provider_folders(Provider::Claude, &home, &state.data_dir);
        for dir in &claude_dirs {
            let projects = dir.join("projects");
            let Ok(real) = std::fs::canonicalize(&projects) else { continue };
            if !seen.insert(real) {
                continue;
            }
            for (old, new) in &nested {
                match rename::move_claude_project(&projects, &slug_for(Path::new(old)), &slug_for(Path::new(new))) {
                    Ok(count) => report.claude_transcripts_moved += count,
                    Err(problem) => report.problems.push(problem),
                }
            }
        }
        let claude_json = std::iter::once(home.join(".claude.json")).chain(claude_dirs.iter().map(|dir| dir.join(".claude.json")));
        let codex_config = provider_folders(Provider::Codex, &home, &state.data_dir).into_iter().map(|dir| dir.join("config.toml"));
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for (path, is_claude) in claude_json.map(|path| (path, true)).chain(codex_config.map(|path| (path, false))) {
            let Ok(real) = std::fs::canonicalize(&path) else { continue };
            if !seen.insert(real) {
                continue;
            }
            let result = if is_claude {
                rename::rewrite_claude_json(&path, &record.canonical_root, &new_name)
            } else {
                rename::rewrite_codex_config(&path, &record.canonical_root, &new_name)
            };
            match result {
                Ok(count) => report.config_entries_updated += count,
                Err(problem) => report.problems.push(problem),
            }
        }
    }

    let root = new_root.clone();
    match tauri::async_runtime::spawn_blocking(move || rename::repair_git_worktrees(&root)).await {
        Ok(Ok(repaired)) => report.git_worktrees_repaired = repaired,
        Ok(Err(problem)) => report.problems.push(problem),
        Err(error) => report.problems.push(error.to_string()),
    }

    tracing::info!(workspace = %workspace_id, transcripts = report.claude_transcripts_moved, configs = report.config_entries_updated, "renamed workspace folder");
    Ok(report)
}
