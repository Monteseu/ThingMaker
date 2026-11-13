//! Renaming a workspace's folder on disk without losing what was done in it.
//!
//! A project's folder name is written in more places than the folder itself:
//! the desktop's records (`Storage::workspace_relocate`), Claude Code's
//! transcript folder (`~/.claude/projects/<slug of the path>`), the
//! per-project entries in `~/.claude.json` and Codex's `config.toml`, and the
//! links git keeps between a repository and its worktrees. This module holds
//! the pieces that touch the agents' files and git; each is best effort and
//! says in words what it could not do, because the folder has already moved
//! by the time they run.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Why a folder name cannot be used, in words, or `None` when it can.
pub fn invalid_folder_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("The name is empty.");
    }
    if name != name.trim() {
        return Some("The name starts or ends with a space.");
    }
    if name == "." || name == ".." {
        return Some("That name is reserved.");
    }
    if name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Some("The name has a character folders cannot use (/ \\ : * ? \" < > |).");
    }
    if name.ends_with('.') {
        return Some("The name ends with a dot.");
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Some("That name is reserved on Windows.");
    }
    if name.len() > 255 {
        return Some("The name is too long.");
    }
    None
}

/// A path as people and agents write it: without Windows' `\\?\` prefix.
pub fn ordinary(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
}

fn normalized(path: &str) -> String {
    let path = ordinary(path).replace('\\', "/");
    let path = path.trim_end_matches('/');
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

/// The same key after the folder `old_root` was renamed to `new_name`, in the
/// key's own spelling (separators, case), or `None` when the key is not the
/// folder or a path inside it.
pub fn renamed_key(key: &str, old_root: &str, new_name: &str) -> Option<String> {
    let old = normalized(old_root);
    let candidate = normalized(key);
    if candidate != old && !candidate.starts_with(&format!("{old}/")) {
        return None;
    }
    // Locate the old folder name inside the key itself, so the parent part
    // keeps the key's spelling: it ends where the normalized old root ends.
    let key_ordinary = ordinary(key);
    let prefix_len = key.len() - key_ordinary.len();
    let old_ordinary = ordinary(old_root);
    let old_ordinary = old_ordinary.trim_end_matches(['/', '\\']);
    let old_name = old_ordinary.rsplit(['/', '\\']).next().unwrap_or(old_ordinary);
    let end = prefix_len + old_ordinary.len();
    let start = end - old_name.len();
    if !key.is_char_boundary(start) || !key.is_char_boundary(end) {
        return None;
    }
    let written = &key[start..end];
    // Codex writes its keys in lower case on Windows; keep doing so.
    let name = if written != old_name && written == old_name.to_lowercase() { new_name.to_lowercase() } else { new_name.to_string() };
    Some(format!("{}{}{}", &key[..start], name, &key[end..]))
}

/// Whether two paths name the same folder on disk: on a case-insensitive
/// disk `Game` and `game` do, on a case-sensitive one they are two folders.
/// Asked of the disk (same file id), never assumed from the platform.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Canonical paths carry the disk's own spelling of each name.
        match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// Renames a folder, never over another one: when `to` already exists it
/// must be `from` itself under another case (a case-only rename on a
/// case-insensitive disk), which goes through a temporary name so the new
/// spelling sticks. Anything else at `to` is refused.
pub fn move_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    if !to.exists() {
        return fs::rename(from, to);
    }
    if !same_folder(from, to) {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} already exists", to.display())));
    }
    let name = to.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = to.with_file_name(format!(".{name}.thingmaker-rename-{}", std::process::id()));
    fs::rename(from, &temp)?;
    fs::rename(&temp, to).inspect_err(|_| {
        let _ = fs::rename(&temp, from);
    })
}

/// Moves Claude Code's transcript folder for a project from one slug to
/// another. When the new one already exists (the new path was opened before),
/// the old folder's entries are moved in beside it; an entry with the same
/// name is left where it was and reported. Returns the entries moved.
pub fn move_claude_project(projects: &Path, old_slug: &str, new_slug: &str) -> Result<usize, String> {
    if old_slug == new_slug {
        return Ok(0);
    }
    let from = projects.join(old_slug);
    if !from.is_dir() {
        return Ok(0);
    }
    let to = projects.join(new_slug);
    // A change of case only may find the folder itself "already there".
    if !to.exists() || same_folder(&from, &to) {
        let count = fs::read_dir(&from).map(|entries| entries.count()).unwrap_or(0);
        move_folder(&from, &to).map_err(|error| format!("{}: {error}", from.display()))?;
        return Ok(count);
    }
    let mut moved = 0;
    let mut kept = Vec::new();
    for entry in fs::read_dir(&from).map_err(|error| format!("{}: {error}", from.display()))?.flatten() {
        let target = to.join(entry.file_name());
        if target.exists() {
            kept.push(entry.file_name().to_string_lossy().into_owned());
            continue;
        }
        fs::rename(entry.path(), &target).map_err(|error| format!("{}: {error}", entry.path().display()))?;
        moved += 1;
    }
    if kept.is_empty() {
        let _ = fs::remove_dir(&from);
        Ok(moved)
    } else {
        Err(format!("{} item(s) were already in {} and stayed in {}: {}", kept.len(), to.display(), from.display(), kept.join(", ")))
    }
}

/// Renames the project entries of a Claude Code `.claude.json` (trust,
/// allowed tools, MCP servers per project). Returns how many were renamed.
pub fn rewrite_claude_json(path: &Path, old_root: &str, new_name: &str) -> Result<usize, String> {
    let Ok(text) = fs::read_to_string(path) else { return Ok(0) };
    let mut value: Value = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let Some(projects) = value.get_mut("projects").and_then(Value::as_object_mut) else { return Ok(0) };
    let renames: Vec<(String, String)> = projects
        .keys()
        .filter_map(|key| renamed_key(key, old_root, new_name).map(|new| (key.clone(), new)))
        .collect();
    let mut count = 0;
    for (old, new) in renames {
        if projects.contains_key(&new) {
            continue;
        }
        if let Some(entry) = projects.remove(&old) {
            projects.insert(new, entry);
            count += 1;
        }
    }
    if count > 0 {
        let pretty = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
        write_replacing(path, pretty.as_bytes())?;
    }
    Ok(count)
}

/// Renames the `[projects."<path>"]` tables of a Codex `config.toml`, keeping
/// the rest of the file as it was written. Returns how many were renamed.
pub fn rewrite_codex_config(path: &Path, old_root: &str, new_name: &str) -> Result<usize, String> {
    let Ok(text) = fs::read_to_string(path) else { return Ok(0) };
    let mut document: toml_edit::DocumentMut = text.parse().map_err(|error| format!("{}: {error}", path.display()))?;
    let Some(projects) = document.get_mut("projects").and_then(toml_edit::Item::as_table_like_mut) else { return Ok(0) };
    let renames: Vec<(String, String)> = projects
        .iter()
        .filter_map(|(key, _)| renamed_key(key, old_root, new_name).map(|new| (key.to_string(), new)))
        .collect();
    let mut count = 0;
    for (old, new) in renames {
        if projects.contains_key(&new) {
            continue;
        }
        if let Some(item) = projects.remove(&old) {
            projects.insert(&new, item);
            count += 1;
        }
    }
    if count > 0 {
        write_replacing(path, document.to_string().as_bytes())?;
    }
    Ok(count)
}

/// Writes beside the file and swaps it in, so a failure never leaves half a
/// configuration file behind. A symbolic link is written through (its target
/// is replaced, the link stays) and the file keeps its permissions.
fn write_replacing(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = real.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = real.with_file_name(format!(".{name}.thingmaker-rename.tmp"));
    let written = fs::write(&temp, bytes).and_then(|()| match fs::metadata(&real) {
        Ok(metadata) => fs::set_permissions(&temp, metadata.permissions()),
        Err(_) => Ok(()),
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("{}: {error}", temp.display()));
    }
    fs::rename(&temp, &real).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("{}: {error}", real.display())
    })
}

/// The worktrees git lists for the checkout at `root`, other than `root`
/// itself, as git recorded them (after a move, still under the old path).
/// Empty outside a git checkout or when git cannot be run.
pub fn git_worktrees(root: &Path) -> Vec<PathBuf> {
    if !root.join(".git").exists() {
        return Vec::new();
    }
    let Ok(list) = crate::platform::command("git").arg("-C").arg(root).args(["worktree", "list", "--porcelain"]).output() else {
        return Vec::new();
    };
    // git lists real paths; the root may be spelt through a link.
    let root_texts = [Some(root.to_path_buf()), fs::canonicalize(root).ok()].map(|path| path.map(|path| normalized(&path.to_string_lossy())));
    String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter(|path| !root_texts.contains(&Some(normalized(path))))
        .map(PathBuf::from)
        .collect()
}

/// What `repair_git_worktrees` did.
#[derive(Debug, Default)]
pub struct WorktreeRepair {
    /// The folder is a git checkout, so its links were looked at.
    pub checkout: bool,
    /// Worktrees re-linked.
    pub repaired: usize,
    /// What could not be re-linked, in words.
    pub problems: Vec<String>,
}

/// Re-links git worktrees after the folder `old_root` was renamed to
/// `new_root`: git keeps absolute paths both ways. A worktree git still lists
/// under the old folder (Claude Code keeps its own in `.claude/worktrees`) is
/// looked for under the new one; each is repaired on its own, so one that
/// cannot be does not leave the others broken, and one no longer on disk is
/// left alone. Nothing to do outside a git checkout.
pub fn repair_git_worktrees(old_root: &Path, new_root: &Path) -> WorktreeRepair {
    let mut report = WorktreeRepair::default();
    if !new_root.join(".git").exists() {
        return report;
    }
    report.checkout = true;
    let repair = |paths: &[&Path]| -> Result<(), String> {
        let output = crate::platform::command("git")
            .arg("-C")
            .arg(new_root)
            .args(["worktree", "repair"])
            .args(paths)
            .output()
            .map_err(|error| format!("git: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("git worktree repair: {}", String::from_utf8_lossy(&output.stderr).trim()))
        }
    };
    // The folder itself first: when it is a linked worktree, this points its
    // repository back at it.
    if let Err(problem) = repair(&[]) {
        report.problems.push(problem);
    }
    let old_text = old_root.to_string_lossy();
    let new_name = new_root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    // git records real paths; the old root may be spelt through a link
    // (macOS' /var is /private/var), so it is also tried from the real parent.
    let real_old = new_root
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .zip(old_root.file_name())
        .map(|(parent, name)| parent.join(name).to_string_lossy().into_owned());
    for listed in git_worktrees(new_root) {
        let listed_text = listed.to_string_lossy().into_owned();
        let moved = renamed_key(&listed_text, &old_text, &new_name).or_else(|| real_old.as_deref().and_then(|real| renamed_key(&listed_text, real, &new_name)));
        let path = moved.map(PathBuf::from).unwrap_or(listed);
        // When the folder is itself a linked worktree, git lists it under
        // its old path: it was repaired above.
        if !path.exists() || same_folder(&path, new_root) {
            continue;
        }
        match repair(&[&path]) {
            Ok(()) => report.repaired += 1,
            Err(problem) => report.problems.push(format!("{}: {problem}", path.display())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_checked() {
        assert!(invalid_folder_name("kid-game").is_none());
        assert!(invalid_folder_name("My Game 2").is_none());
        for bad in ["", " x", "a/b", "a\\b", "..", "x.", "CON", "com1.txt", "a:b"] {
            assert!(invalid_folder_name(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn keys_are_renamed_in_their_own_spelling() {
        let old = if cfg!(windows) { r"\\?\D:\repos\Game" } else { "/repos/Game" };
        let renamed = |key: &str| renamed_key(key, old, "KidGame");
        if cfg!(windows) {
            assert_eq!(renamed(r"D:\repos\Game").as_deref(), Some(r"D:\repos\KidGame"));
            assert_eq!(renamed("D:/repos/Game/tools").as_deref(), Some("D:/repos/KidGame/tools"));
            assert_eq!(renamed(r"d:\repos\game").as_deref(), Some(r"d:\repos\kidgame"));
            assert_eq!(renamed(r"D:\repos\GameOld"), None);
        } else {
            assert_eq!(renamed("/repos/Game").as_deref(), Some("/repos/KidGame"));
            assert_eq!(renamed("/repos/Game/tools").as_deref(), Some("/repos/KidGame/tools"));
            assert_eq!(renamed("/repos/GameOld"), None);
            assert_eq!(renamed("/repos/game"), None);
        }
    }

    #[test]
    fn claude_transcripts_follow_the_folder_and_merge_into_an_existing_one() {
        let temp = tempfile::tempdir().unwrap();
        let projects = temp.path();
        fs::create_dir_all(projects.join("-r-Game")).unwrap();
        fs::write(projects.join("-r-Game/a.jsonl"), "{}").unwrap();
        assert_eq!(move_claude_project(projects, "-r-Game", "-r-Kid").unwrap(), 1);
        assert!(projects.join("-r-Kid/a.jsonl").is_file());
        assert!(!projects.join("-r-Game").exists());

        fs::create_dir_all(projects.join("-r-Old")).unwrap();
        fs::write(projects.join("-r-Old/b.jsonl"), "{}").unwrap();
        fs::write(projects.join("-r-Old/a.jsonl"), "old").unwrap();
        let problem = move_claude_project(projects, "-r-Old", "-r-Kid").unwrap_err();
        assert!(problem.contains("a.jsonl"));
        assert!(projects.join("-r-Kid/b.jsonl").is_file());
        assert_eq!(fs::read_to_string(projects.join("-r-Kid/a.jsonl")).unwrap(), "{}", "an existing transcript is never overwritten");
        assert_eq!(move_claude_project(projects, "-r-Missing", "-r-Kid").unwrap(), 0);
    }

    fn git(dir: &Path, args: &[&str]) -> bool {
        crate::platform::command("git").arg("-C").arg(dir).args(args).output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// A repository `Game` with one commit, or `None` when git is missing.
    fn repository(parent: &Path) -> Option<PathBuf> {
        let repo = parent.join("Game");
        fs::create_dir(&repo).unwrap();
        if !git(&repo, &["init", "-q", "-b", "main"]) {
            return None; // no git here
        }
        fs::write(repo.join("a.txt"), "a").unwrap();
        assert!(git(&repo, &["add", "."]));
        assert!(git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "a"]));
        Some(repo)
    }

    #[test]
    fn worktrees_inside_and_outside_still_work_after_the_repository_is_renamed() {
        let temp = tempfile::tempdir().unwrap();
        let Some(repo) = repository(temp.path()) else { return };
        let outside = temp.path().join("wt");
        // Where Claude Code puts its worktrees: inside the folder that moves.
        assert!(git(&repo, &["worktree", "add", "-q", ".claude/worktrees/agent-1", "-b", "inside"]));
        assert!(git(&repo, &["worktree", "add", "-q", &outside.to_string_lossy(), "-b", "side"]));
        assert_eq!(git_worktrees(&repo).len(), 2);

        let renamed = temp.path().join("KidGame");
        fs::rename(&repo, &renamed).unwrap();
        let inside = renamed.join(".claude/worktrees/agent-1");
        assert!(!git(&outside, &["status"]), "the link is broken before the repair");
        assert!(!git(&inside, &["status"]), "the link is broken before the repair");
        let report = repair_git_worktrees(&repo, &renamed);
        assert!(report.checkout);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.repaired, 2);
        assert!(git(&outside, &["status"]), "the worktree outside finds its repository again");
        assert!(git(&inside, &["status"]), "the worktree inside the folder finds its repository again");
        let listed = git_worktrees(&renamed);
        assert!(listed.iter().any(|path| path.ends_with("KidGame/.claude/worktrees/agent-1")), "{listed:?}");

        let plain = temp.path().join("Plain");
        fs::create_dir(&plain).unwrap();
        assert!(!repair_git_worktrees(&temp.path().join("Before"), &plain).checkout, "nothing to do outside a checkout");
    }

    #[test]
    fn a_missing_worktree_is_skipped_and_the_others_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let Some(repo) = repository(temp.path()) else { return };
        let gone = temp.path().join("gone");
        let kept = temp.path().join("kept");
        assert!(git(&repo, &["worktree", "add", "-q", &gone.to_string_lossy(), "-b", "gone"]));
        assert!(git(&repo, &["worktree", "add", "-q", &kept.to_string_lossy(), "-b", "kept"]));
        fs::remove_dir_all(&gone).unwrap();

        let renamed = temp.path().join("KidGame");
        fs::rename(&repo, &renamed).unwrap();
        let report = repair_git_worktrees(&repo, &renamed);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.repaired, 1);
        assert!(git(&kept, &["status"]));
    }

    #[test]
    fn a_case_only_rename_sticks_and_never_replaces_another_folder() {
        let temp = tempfile::tempdir().unwrap();
        let names = |dir: &Path| fs::read_dir(dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let projects = temp.path().join("projects");
        fs::create_dir_all(projects.join("-r-Game")).unwrap();
        fs::write(projects.join("-r-Game/a.jsonl"), "{}").unwrap();
        assert_eq!(move_claude_project(&projects, "-r-Game", "-r-game").unwrap(), 1);
        assert_eq!(names(&projects), ["-r-game"], "the new spelling is on disk, whatever the disk's case rules");
        assert!(projects.join("-r-game/a.jsonl").is_file());

        let parent = temp.path().join("folders");
        fs::create_dir_all(parent.join("Game")).unwrap();
        move_folder(&parent.join("Game"), &parent.join("game")).unwrap();
        assert_eq!(names(&parent), ["game"]);
        fs::create_dir(parent.join("Other")).unwrap();
        assert!(move_folder(&parent.join("game"), &parent.join("Other")).is_err(), "an empty folder is not replaced");
        assert!(parent.join("game").is_dir() && parent.join("Other").is_dir());
        assert!(!same_folder(&parent.join("game"), &parent.join("Other")));
        assert!(same_folder(&parent.join("game"), &parent.join("game")));
    }

    #[test]
    fn config_entries_are_renamed_and_the_rest_kept() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Game");
        let root_text = root.to_string_lossy().into_owned();
        let other = temp.path().join("Other").to_string_lossy().into_owned();

        let claude = temp.path().join(".claude.json");
        let json = serde_json::json!({ "numStartups": 3, "projects": { root_text.clone(): { "hasTrustDialogAccepted": true }, other.clone(): {} } });
        fs::write(&claude, serde_json::to_string(&json).unwrap()).unwrap();
        assert_eq!(rewrite_claude_json(&claude, &root_text, "Kid").unwrap(), 1);
        let after: Value = serde_json::from_str(&fs::read_to_string(&claude).unwrap()).unwrap();
        let new_key = temp.path().join("Kid").to_string_lossy().into_owned();
        assert_eq!(after["projects"][&new_key]["hasTrustDialogAccepted"], true);
        assert!(after["projects"].get(&root_text).is_none());
        assert!(after["projects"].get(&other).is_some());
        assert_eq!(after["numStartups"], 3);

        let codex = temp.path().join("config.toml");
        fs::write(&codex, format!("model = \"x\" # mine\n\n[projects.'{root_text}']\ntrust_level = \"trusted\"\n\n[projects.'{other}']\ntrust_level = \"trusted\"\n")).unwrap();
        assert_eq!(rewrite_codex_config(&codex, &root_text, "Kid").unwrap(), 1);
        let text = fs::read_to_string(&codex).unwrap();
        assert!(text.contains("# mine"), "comments survive: {text}");
        let parsed: toml::Table = text.parse().unwrap();
        assert!(parsed["projects"].get(&new_key).is_some(), "{text}");
        assert!(parsed["projects"].get(&root_text).is_none());
        assert!(parsed["projects"].get(&other).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_config_is_written_through_and_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root_text = temp.path().join("Game").to_string_lossy().into_owned();
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join("claude.json");
        fs::write(&real, serde_json::json!({ "projects": { root_text.clone(): {} } }).to_string()).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join(".claude.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(rewrite_claude_json(&link, &root_text, "Kid").unwrap(), 1);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        assert!(fs::read_to_string(&real).unwrap().contains("Kid"), "the linked file got the change");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_dir(&dotfiles).unwrap().count(), 1, "no temporary file left behind");
    }
}
