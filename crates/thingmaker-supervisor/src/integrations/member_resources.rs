//! Shared per-worker catalog. Public entries contain no MCP configuration
//! or credentials; resolution happens again in the host immediately before launch.
use std::{collections::{BTreeMap, HashMap}, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{DesktopError, storage::content_hash};

mod skill_dependencies;
pub use skill_dependencies::McpDependency;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
// No deny_unknown_fields: a field added later must not make an older build
// reject the whole team that carries it.
#[serde(rename_all = "camelCase")]
pub struct ResourceSelection {
    pub skills: Vec<String>,
    pub mcps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub group: String,
    pub available: bool,
    pub problem: Option<String>,
    pub source: String,
    /// What an MCP server runs, or the host it connects to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub mcp_dependencies: Vec<McpDependency>,
    pub dependency_notes: Vec<String>,
    #[serde(skip)]
    runtime_name: String,
    #[serde(skip)]
    config: Value,
    #[serde(skip)]
    revision: String,
    #[serde(skip)]
    skill_path: Option<PathBuf>,
    #[serde(skip)]
    bridge_skill: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCatalog {
    pub entries: Vec<ResourceEntry>,
    pub notes: Vec<String>,
}

impl ResourceCatalog {
    /// Candidate IDs remain visible, including incompatible definitions, so
    /// the selector can explain why a dependency cannot be activated here.
    pub fn for_provider(mut self, provider: crate::agents::Provider) -> Self {
        if provider == crate::agents::Provider::Codex {
            for entry in self.entries.iter_mut().filter(|entry| entry.kind == "mcp" && entry.config["type"] == "sse") {
                entry.available = false;
                entry.problem = Some("SSE MCP transport is not supported by Codex".into());
            }
        }
        self
    }
}

fn read(path: &Path) -> Result<Value, DesktopError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| DesktopError::io(format!("Invalid JSON in {}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err(DesktopError::io(format!("Cannot read {}", path.display()))),
    }
}

fn description(text: &str, parsed: Option<String>) -> String {
    let parsed = parsed.unwrap_or_default();
    if ![">", ">-", "|", "|-"].contains(&parsed.as_str()) { return parsed }
    let lines: Vec<&str> = text.lines().skip_while(|line| !line.starts_with("description:")).skip(1)
        .take_while(|line| line.starts_with(' ') || line.starts_with('\t') || line.is_empty()).map(str::trim).collect();
    lines.join(if parsed.starts_with('|') { "\n" } else { " " }).trim().to_string()
}

fn add_skills(out: &mut Vec<ResourceEntry>, directory: &Path, group: &str, prefix: &str, plugin: Option<&str>, enabled: bool) {
    let directories: Vec<PathBuf> = if directory.join("SKILL.md").is_file() { vec![directory.to_path_buf()] }
        else { std::fs::read_dir(directory).into_iter().flatten().flatten().map(|entry| entry.path()).collect() };
    for directory in directories {
        let file = directory.join("SKILL.md");
        let Ok(bytes) = std::fs::read(&file) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let front = super::skills::parse_frontmatter(&text);
        let Some(name) = directory.file_name().map(|name| name.to_string_lossy().into_owned()) else { continue };
        let runtime_name = plugin.map(|plugin| {
            let skill = front.name.as_deref().unwrap_or(&name);
            if skill.starts_with(&format!("{plugin}:")) { skill.to_string() } else { format!("{plugin}:{skill}") }
        }).unwrap_or_else(|| name.clone());
        let disabled = front.other.get("disable-model-invocation").is_some_and(|value| value == "true") || front.other.get("context").is_some_and(|value| value == "fork");
        let revision = content_hash(&bytes);
        out.push(ResourceEntry { id: format!("skill:{prefix}:{name}"), kind: "skill".into(), name: runtime_name.clone(),
            description: description(&text, front.description), group: group.into(), available: enabled && !disabled,
            problem: if !enabled { Some("Plugin disabled for this project".into()) } else { disabled.then(|| "Manual-only or forked skill; not supported by the member pilot".into()) }, runtime_name, config: Value::Null,
            revision: format!("{}:{revision}", file.display()), source: if plugin.is_some() { "plugin".into() } else { ".claude/skills".into() },
            skill_path: Some(file), bridge_skill: false, target: None, mcp_dependencies: Vec::new(), dependency_notes: Vec::new() });
    }
}

fn add_mcps(out: &mut Vec<ResourceEntry>, value: &Value, group: &str, prefix: &str, plugin: Option<&str>, enabled: bool, plugin_root: Option<&Path>) {
    let servers = value.get("mcpServers").unwrap_or(value);
    let Some(servers) = servers.as_object() else { return };
    for (name, config) in servers {
        let valid = config.get("command").is_some_and(Value::is_string) || config.get("url").is_some_and(Value::is_string);
        let available = enabled && valid && config.get("disabled").and_then(Value::as_bool) != Some(true);
        let runtime_name = plugin.map(|plugin| format!("plugin:{plugin}:{name}")).unwrap_or_else(|| name.clone());
        let mut config = config.clone();
        if let Some(root) = plugin_root {
            replace_plugin_root(&mut config, &root.to_string_lossy());
        }
        out.push(ResourceEntry { id: format!("mcp:{prefix}:{name}"), kind: "mcp".into(), name: runtime_name.clone(),
            // Never put full URLs, arguments, environment or headers in the
            // public catalog: only the program or the host, in `target`.
            description: "MCP server".into(), group: group.into(), available, target: server_target(&config),
            problem: (!available).then(|| "Server disabled or unsupported transport".into()), runtime_name,
            revision: content_hash(serde_json::to_string(&config).unwrap_or_default().as_bytes()), config, source: if plugin.is_some() { "plugin".into() } else { ".claude".into() }, skill_path: None, bridge_skill: false, mcp_dependencies: Vec::new(), dependency_notes: Vec::new() });
    }
}

/// What a server runs or where it connects, so the person can see what they
/// are choosing: the program, or the URL's host. Never arguments, headers,
/// environment, user info or query, which can carry credentials.
fn server_target(config: &Value) -> Option<String> {
    if let Some(command) = config.get("command").and_then(Value::as_str) {
        return Some(command.to_string()).filter(|command| !command.is_empty());
    }
    let url = config.get("url").and_then(Value::as_str)?;
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_string())
}

/// Claude Code's own approval of a project's shared `.mcp.json` servers: a
/// server it would not start is never offered. Approvals count only from the
/// person's own files (user settings, `settings.local.json`, the project's
/// entry in `.claude.json`), never from the committed `.claude/settings.json`,
/// which the project's authors control; refusals count from all of them.
#[derive(Default)]
struct ProjectServerApproval {
    all: bool,
    enabled: Vec<String>,
    disabled: Vec<String>,
}

impl ProjectServerApproval {
    fn read(&mut self, value: &Value, trusted: bool) {
        let names = |key: &str| value.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        if trusted {
            self.all |= value.get("enableAllProjectMcpServers").and_then(Value::as_bool) == Some(true);
            self.enabled.extend(names("enabledMcpjsonServers"));
        }
        self.disabled.extend(names("disabledMcpjsonServers"));
    }

    fn problem(&self, name: &str) -> Option<&'static str> {
        if self.disabled.iter().any(|disabled| disabled == name) {
            Some("Turned off for this project in Claude Code")
        } else if !self.all && !self.enabled.iter().any(|enabled| enabled == name) {
            Some("Project server not approved in Claude Code. Approve it there, then refresh.")
        } else {
            None
        }
    }
}

fn replace_plugin_root(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace("${CLAUDE_PLUGIN_ROOT}", root),
        Value::Array(items) => items.iter_mut().for_each(|item| replace_plugin_root(item, root)),
        Value::Object(items) => items.values_mut().for_each(|item| replace_plugin_root(item, root)),
        _ => {}
    }
}

fn add_codex_mcps(out: &mut Vec<ResourceEntry>, file: &Path, group: &str, prefix: &str) -> Result<(), DesktopError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DesktopError::io("Cannot read Codex MCP configuration")),
    };
    let config: toml::Table = text.parse().map_err(|_| DesktopError::io("Invalid Codex MCP configuration"))?;
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else { return Ok(()) };
    let mut normalized = Map::new();
    for (name, raw) in servers {
        let mut server = serde_json::to_value(raw).map_err(|_| DesktopError::io("Invalid Codex MCP server"))?;
        let Some(server) = server.as_object_mut() else { continue };
        if server.get("enabled") == Some(&json!(false)) { server.insert("disabled".into(), json!(true)); }
        if server.contains_key("url") {
            server.insert("type".into(), json!("http"));
            let mut headers = server.remove("http_headers").and_then(|value| value.as_object().cloned()).unwrap_or_default();
            if let Some(token) = server.get("bearer_token_env_var").and_then(Value::as_str) { headers.insert("Authorization".into(), json!(format!("Bearer ${{{token}}}"))); }
            server.insert("headers".into(), Value::Object(headers));
        }
        if let Some(names) = server.remove("env_vars").and_then(|value| value.as_array().cloned()) {
            let env = server.entry("env").or_insert_with(|| json!({})).as_object_mut().ok_or_else(|| DesktopError::io("Invalid Codex MCP environment"))?;
            for name in names.iter().filter_map(Value::as_str) { env.entry(name).or_insert_with(|| json!(format!("${{{name}}}"))); }
        }
        normalized.insert(name.clone(), Value::Object(server.clone()));
    }
    let start = out.len();
    add_mcps(out, &Value::Object(normalized), group, prefix, None, true, None);
    for entry in &mut out[start..] { entry.source = ".codex/config.toml".into(); }
    Ok(())
}

/// Account config directory, plus the project the member will work in. Project
/// identifiers deliberately omit absolute paths so exported templates can move.
pub fn catalog(home: &Path, config_dir: &Path, root: Option<&Path>) -> Result<ResourceCatalog, DesktopError> {
    catalog_with_homes(home, config_dir, &home.join(".codex"), root)
}

pub fn catalog_with_homes(home: &Path, config_dir: &Path, codex_home: &Path, root: Option<&Path>) -> Result<ResourceCatalog, DesktopError> {
    let mut entries = Vec::new();
    let mut settings = read(&config_dir.join("settings.json"))?;
    let mut project_dirs = Vec::new();
    if let Some(root) = root {
        // The project's folders: up to its repository's top, or the workspace
        // alone outside a repository. Never $HOME or above it: its .claude and
        // .codex are the person's own, already read as user resources.
        let real_home = home.canonicalize().ok();
        let below_home = |directory: &&Path| *directory != home && Some(*directory) != real_home.as_deref();
        let top = root.ancestors().take_while(below_home).find(|directory| directory.join(".git").exists()).unwrap_or(root);
        for directory in root.ancestors().take_while(below_home) {
            project_dirs.push(directory.to_path_buf());
            if directory == top { break }
        }
        project_dirs.reverse();
    }
    let mut enabled_plugins = BTreeMap::new();
    let mut disabled_mcps = Vec::new();
    let mut skill_overrides = BTreeMap::new();
    let mut approval = ProjectServerApproval::default();
    let mut merge = |value: &Value, trusted: bool| {
        approval.read(value, trusted);
        if let Some(plugins) = value.get("enabledPlugins").and_then(Value::as_object) {
            for (name, enabled) in plugins { enabled_plugins.insert(name.clone(), enabled.as_bool().unwrap_or(false)); }
        }
        if let Some(disabled) = value.get("disabledMcpServers").and_then(Value::as_array) {
            disabled_mcps.extend(disabled.iter().filter_map(Value::as_str).map(str::to_string));
        }
        if let Some(overrides) = value.get("skillOverrides").and_then(Value::as_object) {
            for (name, mode) in overrides { skill_overrides.insert(name.clone(), mode.as_str().unwrap_or("on").to_string()); }
        }
    };
    merge(&settings, true);
    for directory in &project_dirs {
        for (name, trusted) in [("settings.json", false), ("settings.local.json", true)] { merge(&read(&directory.join(".claude").join(name))?, trusted); }
    }
    // The default account stores .claude.json beside ~/.claude; added accounts
    // have their own merged copy inside CLAUDE_CONFIG_DIR.
    let account_file = config_dir.join(".claude.json");
    let default_file = home.join(".claude.json");
    settings = read(if account_file.exists() { &account_file } else { &default_file })?;
    add_skills(&mut entries, &config_dir.join("skills"), "user", "user", None, true);
    add_external_skills(&mut entries, &home.join(".agents/skills"), "user", "agents-user", ".agents/skills");
    add_external_skills(&mut entries, &codex_home.join("skills"), "user", "codex-user", ".codex/skills");
    add_external_skills(&mut entries, &codex_home.join("skills/.system"), "user", "codex-system", ".codex/skills/.system");
    add_mcps(&mut entries, &settings["mcpServers"], "user", "user", None, true, None);
    add_codex_mcps(&mut entries, &codex_home.join("config.toml"), "user", "codex-user")?;
    for directory in &project_dirs {
        let relative = root.and_then(|root| root.strip_prefix(directory).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let prefix = if relative.is_empty() { "project".into() } else { format!("project-parent:{relative}") };
        add_skills(&mut entries, &directory.join(".claude/skills"), "project", &prefix, None, true);
        add_external_skills(&mut entries, &directory.join(".agents/skills"), "project", &format!("agents-{prefix}"), ".agents/skills");
        add_external_skills(&mut entries, &directory.join(".codex/skills"), "project", &format!("codex-{prefix}"), ".codex/skills");
        add_codex_mcps(&mut entries, &directory.join(".codex/config.toml"), "project", &format!("codex-{prefix}"))?;
    }
    if let Some(root) = root {
        let local = &settings["projects"][root.to_string_lossy().as_ref()];
        approval.read(local, true);
        let start = entries.len();
        add_mcps(&mut entries, &read(&root.join(".mcp.json"))?, "project", "project", None, true, None);
        // Before precedence: an unapproved project server replaces nothing.
        for entry in entries[start..].iter_mut().filter(|entry| entry.available) {
            if let Some(problem) = approval.problem(&entry.runtime_name) {
                entry.available = false;
                entry.problem = Some(problem.into());
            }
        }
        add_mcps(&mut entries, &local["mcpServers"], "project", "local", None, true, None);
        if let Some(disabled) = local["disabledMcpServers"].as_array() {
            disabled_mcps.extend(disabled.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    let registry = read(&config_dir.join("plugins/installed_plugins.json"))?;
    if registry.get("plugins").is_some() && registry["version"] != 2 {
        return Err(DesktopError::unsupported("Unsupported Claude plugin registry version"));
    }
    if let Some(plugins) = registry["plugins"].as_object() {
        for (id, installs) in plugins {
            let Some(installs) = installs.as_array() else { continue };
            let chosen = installs.iter().rev().find(|install| install["projectPath"].as_str().is_some_and(|path| root.is_some_and(|root| root == Path::new(path))))
                .or_else(|| installs.iter().rev().find(|install| install["scope"] == "user" || install["scope"] == "managed"));
            let Some(path) = chosen.and_then(|install| install["installPath"].as_str()).map(PathBuf::from) else { continue };
            let manifest = read(&path.join(".claude-plugin/plugin.json"))?;
            let name = manifest["name"].as_str().unwrap_or_else(|| id.split('@').next().unwrap_or(id));
            let group = format!("plugin:{id}");
            let enabled = enabled_plugins.get(id).copied().unwrap_or(false);
            if path.join("SKILL.md").is_file() { add_skills(&mut entries, &path, &group, id, Some(name), enabled); }
            add_skills(&mut entries, &path.join("skills"), &group, id, Some(name), enabled);
            if let Some(paths) = manifest.get("skills") {
                for custom in paths.as_array().cloned().unwrap_or_else(|| vec![paths.clone()]) {
                    if let Some(custom) = custom.as_str() { add_skills(&mut entries, &path.join(custom), &group, id, Some(name), enabled); }
                }
            }
            add_mcps(&mut entries, &read(&path.join(".mcp.json"))?, &group, id, Some(name), enabled, Some(&path));
            if let Some(custom) = manifest.get("mcpServers") {
                if custom.is_object() { add_mcps(&mut entries, custom, &group, id, Some(name), enabled, Some(&path)); }
                else {
                    for custom in custom.as_array().cloned().unwrap_or_else(|| vec![custom.clone()]) {
                        if let Some(custom) = custom.as_str() { add_mcps(&mut entries, &read(&path.join(custom))?, &group, id, Some(name), enabled, Some(&path)); }
                    }
                }
            }
        }
    }
    // Later native definitions have precedence. A hidden duplicate cannot be
    // selected by origin when Claude itself resolves both by the same name.
    // The same manifest can explicitly list its default directories/files.
    // Keep its last occurrence before computing precedence.
    let mut unique = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() { unique.insert(entry.id.clone(), index); }
    entries = entries.into_iter().enumerate().filter(|(index, entry)| unique.get(&entry.id) == Some(index)).map(|(_, entry)| entry).collect();
    let mut winners = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.available {
            let key = (entry.kind.clone(), entry.runtime_name.clone(), entry.source.starts_with(".codex/config"));
            let user_skill_wins = entry.kind == "skill" && winners.get(&key).is_some_and(|previous: &usize| entries[*previous].group == "user");
            if !user_skill_wins { winners.insert(key, index); }
        }
    }
    for (index, entry) in entries.iter_mut().enumerate() {
        if entry.kind == "skill" && !entry.bridge_skill && !entry.group.starts_with("plugin:") && skill_overrides.get(&entry.runtime_name).is_some_and(|mode| mode == "off" || mode == "user-invocable-only") {
            entry.available = false;
            entry.problem = Some("Skill disabled for model invocation in Claude settings".into());
        } else if entry.kind == "mcp" && !entry.source.starts_with(".codex/config") && disabled_mcps.contains(&entry.runtime_name) {
            entry.available = false;
            entry.problem = Some("Server disabled for this project".into());
        } else if entry.available && winners.get(&(entry.kind.clone(), entry.runtime_name.clone(), entry.source.starts_with(".codex/config"))) != Some(&index) {
            entry.available = false;
            entry.problem = Some("Replaced by another definition of the same name".into());
        }
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    skill_dependencies::annotate(&mut entries);
    Ok(ResourceCatalog { entries, notes: vec![
        "Shared skills: .claude, .agents and .codex folders, plus installed Claude plugins. Legacy commands, enterprise/synced skills and cloud connectors are not listed. Skills may depend on tools or syntax specific to their original harness.".into(),
        "Selection controls model-invoked skills and MCP servers. File access, slash commands, plugin hooks and managed policies are separate.".into(),
    ] })
}

pub struct ResolvedResources {
    pub skills: Vec<String>,
    pub mcps: BTreeMap<String, Value>,
    pub revision: String,
    pub skill_sources: Vec<SelectedSkill>,
}

pub struct SelectedSkill {
    pub path: PathBuf,
    pub runtime_name: String,
    pub bridge: bool,
    pub name: String,
    pub description: String,
}

pub fn codex_instructions(resources: &ResolvedResources) -> String {
    let mut text = String::from("\n\n<skills_instructions>\n## Available skills\nThis member's skill catalog is fixed for this session. To use a listed skill, read its SKILL.md at the provided path. Resolve its supporting files relative to that skill directory. Skill files may contain instructions specific to their original harness; use only tools actually available here.\n");
    for skill in &resources.skill_sources {
        text.push_str(&format!("- {}\n", json!({"name":skill.name,"description":skill.description,"path":skill.path})));
    }
    text.push_str("</skills_instructions>\n");
    text
}

/// Applies one resolved snapshot to the provider launch, returning ownership of
/// any temporary bridges so the session actor can keep them alive.
pub fn apply_to_launch(resources: ResolvedResources, launch: &mut crate::agents::AgentLaunch) -> Result<Option<std::sync::Arc<tempfile::TempDir>>, DesktopError> {
    match launch {
        crate::agents::AgentLaunch::Claude(options) => {
            let prepared = prepare_claude_skills(&resources)?;
            if let Some(prepared) = &prepared { options.resource_plugins = prepared.plugins.clone(); }
            options.resources = Some((resources.skills, resources.mcps));
            Ok(prepared.map(|prepared| prepared.files))
        }
        crate::agents::AgentLaunch::Codex(options) => {
            let instructions = codex_instructions(&resources);
            options.developer_instructions = Some(format!("{}{instructions}", options.developer_instructions.as_deref().unwrap_or_default()));
            options.resources = Some(crate::agents::codex::launch::CodexResources {
                skills: resources.skill_sources.into_iter().map(|skill| skill.path).collect(), mcps: crate::agents::codex::resources::server_names(resources.mcps),
            });
            Ok(None)
        }
        _ => Err(DesktopError::unsupported("This harness does not support member resources")),
    }
}

pub struct PreparedClaudeSkills {
    pub files: std::sync::Arc<tempfile::TempDir>,
    pub plugins: Vec<PathBuf>,
}

fn add_external_skills(out: &mut Vec<ResourceEntry>, directory: &Path, group: &str, prefix: &str, source: &str) {
    let start = out.len();
    add_skills(out, directory, group, prefix, None, true);
    for entry in &mut out[start..] {
        let original = entry.skill_path.as_ref().and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| super::skills::parse_frontmatter(&text).name).unwrap_or_else(|| entry.runtime_name.clone());
        let plugin = format!("tm-{}", &content_hash(entry.id.as_bytes())[..16]);
        entry.runtime_name = format!("{plugin}:{original}");
        entry.name = original;
        entry.source = source.into();
        entry.bridge_skill = true;
    }
}

/// Only selected external directories are linked into ephemeral, skill-only
/// plugins. Their scripts/resources stay together and no global files change.
pub fn prepare_claude_skills(resources: &ResolvedResources) -> Result<Option<PreparedClaudeSkills>, DesktopError> {
    let external: Vec<_> = resources.skill_sources.iter().filter(|skill| skill.bridge).collect();
    if external.is_empty() { return Ok(None) }
    let temp = std::sync::Arc::new(tempfile::Builder::new().prefix("thingmaker-member-skills-").tempdir().map_err(|_| DesktopError::io("Cannot prepare member skills"))?);
    let mut plugins = Vec::new();
    for skill in external {
        let (plugin_name, _) = skill.runtime_name.split_once(':').ok_or_else(|| DesktopError::io("Invalid bridged skill name"))?;
        let plugin = temp.path().join(plugin_name);
        let source = skill.path.parent().ok_or_else(|| DesktopError::io("Invalid skill directory"))?;
        let name = source.file_name().ok_or_else(|| DesktopError::io("Invalid skill directory name"))?;
        std::fs::create_dir_all(plugin.join(".claude-plugin")).and_then(|_| std::fs::create_dir_all(plugin.join("skills")))
            .and_then(|_| std::fs::write(plugin.join(".claude-plugin/plugin.json"), json!({"name":plugin_name,"version":"1.0.0"}).to_string()))
            .map_err(|_| DesktopError::io("Cannot create temporary member plugin"))?;
        let target = source.canonicalize().map_err(|_| DesktopError::not_ready("Selected skill directory is no longer available"))?;
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, plugin.join("skills").join(name));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&target, plugin.join("skills").join(name));
        linked.map_err(|_| DesktopError::io("Cannot link the selected skill into its temporary plugin"))?;
        plugins.push(plugin);
    }
    Ok(Some(PreparedClaudeSkills { files: temp, plugins }))
}

pub fn resolve(catalog: &ResourceCatalog, selection: &ResourceSelection, environment: &HashMap<String, String>) -> Result<ResolvedResources, DesktopError> {
    let mut skills = Vec::new();
    let mut mcps = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    let mut skill_sources = Vec::new();
    let mut selected_paths = std::collections::BTreeSet::new();
    for entry in catalog.entries.iter().filter(|entry| entry.kind == "skill" && selection.skills.contains(&entry.id)) {
        for dependency in entry.mcp_dependencies.iter().filter(|dependency| dependency.required) {
            if !dependency.mcp_ids.iter().any(|id| selection.mcps.contains(id)) {
                return Err(DesktopError::not_ready(format!("Skill {} requires MCP {}. Enable a matching connection in this member's selector.", entry.name, dependency.name)));
            }
        }
    }
    for (kind, ids) in [("skill", &selection.skills), ("mcp", &selection.mcps)] {
        for id in ids {
            let entry = catalog.entries.iter().find(|entry| entry.id == *id && entry.kind == kind && entry.available)
                .ok_or_else(|| DesktopError::not_ready(format!("Selected resource {id} is missing or disabled. Edit this member's selection.")))?;
            revisions.insert(id, &entry.revision);
            if kind == "skill" {
                let path = entry.skill_path.as_ref().and_then(|path| path.canonicalize().ok()).ok_or_else(|| DesktopError::not_ready("Selected skill file is no longer available"))?;
                // A symlinked installation can expose the same physical skill
                // through several origins; load it once, preferring the first selection.
                if selected_paths.insert(path.clone()) {
                    skills.push(entry.runtime_name.clone());
                    skill_sources.push(SelectedSkill { path, runtime_name: entry.runtime_name.clone(), bridge: entry.bridge_skill, name: entry.name.clone(), description: entry.description.clone() });
                }
            }
            else {
                let mut config = entry.config.clone();
                expand_environment(&mut config, environment)?;
                if mcps.insert(entry.runtime_name.clone(), config).is_some() {
                    return Err(DesktopError::conflict("Two selected MCP definitions use the same server name. Select one origin for that server."));
                }
            }
        }
    }
    skills.sort();
    skills.dedup();
    let revision = content_hash(serde_json::to_vec(&json!([revisions, mcps])).unwrap_or_default().as_slice());
    Ok(ResolvedResources { skills, mcps, revision, skill_sources })
}

fn expand_environment(value: &mut Value, env: &HashMap<String, String>) -> Result<(), DesktopError> {
    match value {
        Value::String(text) => {
            let mut expanded = String::new();
            let mut remaining = text.as_str();
            while let Some(start) = remaining.find("${") {
                expanded.push_str(&remaining[..start]);
                let tail = &remaining[start + 2..];
                let Some(end) = tail.find('}') else { return Err(DesktopError::not_ready("Invalid MCP environment reference")) };
                let (name, default) = tail[..end].split_once(":-").map_or((&tail[..end], None), |(name, default)| (name, Some(default)));
                let replacement = env.get(name).map(String::as_str).or(default).ok_or_else(|| DesktopError::not_ready(format!("Selected MCP needs environment variable {name}")))?;
                expanded.push_str(replacement);
                remaining = &tail[end + 1..];
            }
            expanded.push_str(remaining);
            *text = expanded;
        }
        Value::Array(items) => for item in items { expand_environment(item, env)?; },
        Value::Object(items) => for item in items.values_mut() { expand_environment(item, env)?; },
        _ => {}
    }
    Ok(())
}

/// Fail closed on older installations: this pilot was verified with these
/// adapter/SDK versions, rather than assuming unknown versions honor options.
pub fn check_harness(adapter_script: &Path) -> Result<(), DesktopError> {
    let package = adapter_script.parent().and_then(Path::parent).ok_or_else(|| DesktopError::not_ready("Cannot locate Claude ACP package"))?;
    for (file, minimum) in [(package.join("package.json"), (0, 106, 0)), (package.join("node_modules/@anthropic-ai/claude-agent-sdk/package.json"), (0, 3, 322))] {
        let value = read(&file)?;
        let version = value["version"].as_str().unwrap_or("");
        let parts: Vec<u64> = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
        if parts.len() != 3 || (parts[0], parts[1], parts[2]) < minimum {
            return Err(DesktopError::not_ready("Custom member resources require Claude ACP 0.106.0+ and Agent SDK 0.3.322+. Update the adapter before starting this member."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn origins_collisions_and_disabled_plugins_are_visible_without_server_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let skill = "---\nname: draw\ndescription: Draw pictures\n---\n";
        write(&config.join("skills/draw/SKILL.md"), skill);
        write(&root.join(".claude/skills/draw/SKILL.md"), skill);
        write(&home.join(".claude.json"), r#"{"mcpServers":{"files":{"command":"npx","env":{"TOKEN":"secret-value"}}}}"#);
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art","skills":["./skills"]}"#);
        write(&plugin.join("skills/draw/SKILL.md"), skill);
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        let found = catalog(&home, &config, Some(&root)).unwrap();
        assert!(!serde_json::to_string(&found).unwrap().contains("secret-value"));
        assert!(found.entries.iter().find(|entry| entry.id == "skill:user:draw").unwrap().available);
        assert!(!found.entries.iter().find(|entry| entry.id == "skill:project:draw").unwrap().available);
        let plugin_skill = found.entries.iter().filter(|entry| entry.id == "skill:art@market:draw").collect::<Vec<_>>();
        assert_eq!(plugin_skill.len(), 1);
        assert!(!plugin_skill[0].available);
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        let enabled = catalog(&home, &config, Some(&root)).unwrap();
        assert!(enabled.entries.iter().find(|entry| entry.id == "skill:art@market:draw").unwrap().available);
    }

    #[test]
    fn explicit_empty_missing_and_environment_resolution_never_widen_selection() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join(".claude");
        write(&temp.path().join(".claude.json"), r#"{"mcpServers":{"files":{"command":"npx","env":{"TOKEN":"${TOKEN}"}}}}"#);
        let found = catalog(temp.path(), &config, None).unwrap();
        let empty = resolve(&found, &ResourceSelection::default(), &HashMap::new()).unwrap();
        assert!(empty.skills.is_empty() && empty.mcps.is_empty());
        let selected = ResourceSelection { skills: vec![], mcps: vec!["mcp:user:files".into()] };
        assert!(resolve(&found, &selected, &HashMap::new()).is_err());
        let resolved = resolve(&found, &selected, &HashMap::from([("TOKEN".into(), "credential".into())])).unwrap();
        assert_eq!(resolved.mcps["files"]["env"]["TOKEN"], "credential");
        let changed = resolve(&found, &selected, &HashMap::from([("TOKEN".into(), "rotated".into())])).unwrap();
        assert_ne!(resolved.revision, changed.revision);
        let missing = ResourceSelection { skills: vec!["skill:project:missing".into()], mcps: vec![] };
        assert!(resolve(&found, &missing, &HashMap::new()).is_err());
    }

    #[test]
    fn plugin_mcps_expand_their_root_and_local_project_servers_override_user_servers() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&temp.path().join(".claude.json"), &json!({"mcpServers":{"same":{"command":"user"}},"projects":{root.to_string_lossy().as_ref():{"mcpServers":{"same":{"command":"local"}}}}}).to_string());
        write(&root.join(".mcp.json"), r#"{"mcpServers":{"same":{"command":"project"}}}"#);
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&plugin.join(".mcp.json"), r#"{"paint":{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/server.js"]}}"#);
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        let found = catalog(temp.path(), &config, Some(&root)).unwrap();
        let selected = ResourceSelection { skills: vec![], mcps: vec!["mcp:local:same".into(), "mcp:art@market:paint".into()] };
        let resolved = resolve(&found, &selected, &HashMap::new()).unwrap();
        assert_eq!(resolved.mcps["same"]["command"], "local");
        assert_eq!(resolved.mcps["plugin:art:paint"]["args"][0], plugin.join("server.js").to_string_lossy().as_ref());
        assert!(!found.entries.iter().find(|entry| entry.id == "mcp:user:same").unwrap().available);
    }

    #[test]
    fn missing_or_old_harness_is_rejected_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("dist/index.js");
        assert!(check_harness(&script).is_err());
        write(&temp.path().join("package.json"), r#"{"version":"0.106.0"}"#);
        write(&temp.path().join("node_modules/@anthropic-ai/claude-agent-sdk/package.json"), r#"{"version":"0.3.322"}"#);
        assert!(check_harness(&script).is_ok());
    }

    #[test]
    fn folded_descriptions_and_plugin_frontmatter_names_match_the_catalog() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("directory/SKILL.md"), "---\nname: art:paint\ndescription: >-\n  Paint pictures\n  in color\n---\nBody\n");
        let mut entries = Vec::new();
        add_skills(&mut entries, temp.path(), "plugin:art@market", "art@market", Some("art"), true);
        assert_eq!(entries[0].name, "art:paint");
        assert_eq!(entries[0].description, "Paint pictures in color");
    }

    #[test]
    fn shared_user_project_and_system_skills_keep_their_sources_and_supporting_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let body = "---\nname: art\ndescription: Draw\n---\nRead palette.txt\n";
        for source in [home.join(".agents/skills/art"), home.join(".codex/skills/art"), root.join(".agents/skills/art"), root.join(".codex/skills/art"), home.join(".codex/skills/.system/art")] {
            write(&source.join("SKILL.md"), body);
            write(&source.join("palette.txt"), "palette-marker");
        }
        let found = catalog(&home, &home.join(".claude"), Some(&root)).unwrap();
        for id in ["skill:agents-user:art", "skill:codex-user:art", "skill:agents-project:art", "skill:codex-project:art", "skill:codex-system:art"] {
            assert!(found.entries.iter().any(|entry| entry.id == id && entry.available));
        }
        let chosen = ResourceSelection { skills: vec!["skill:agents-user:art".into(), "skill:codex-project:art".into()], mcps: vec![] };
        let resolved = resolve(&found, &chosen, &HashMap::new()).unwrap();
        assert_ne!(resolved.skills[0], resolved.skills[1], "same-named skills get separate bridge namespaces");
        let prepared = prepare_claude_skills(&resolved).unwrap().unwrap();
        let directory = prepared.files.path().to_path_buf();
        assert_eq!(prepared.plugins.len(), 2);
        for plugin in &prepared.plugins {
            assert_eq!(std::fs::read_to_string(plugin.join("skills/art/palette.txt")).unwrap(), "palette-marker");
        }
        drop(prepared);
        assert!(!directory.exists(), "temporary bridges are removed with their owner");
        assert!(home.join(".agents/skills/art/palette.txt").exists());
    }

    #[test]
    fn native_codex_mcp_definitions_are_portable_but_same_named_origins_require_a_choice() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".claude.json"), r#"{"mcpServers":{"paint":{"command":"claude-paint"}}}"#);
        write(&temp.path().join(".codex/config.toml"), "[mcp_servers.paint]\nurl = \"https://paint.test/mcp\"\nbearer_token_env_var = \"PAINT_TOKEN\"\n");
        let found = catalog(temp.path(), &temp.path().join(".claude"), None).unwrap();
        assert!(found.entries.iter().filter(|entry| entry.kind == "mcp").all(|entry| entry.available));
        let environment = HashMap::from([("PAINT_TOKEN".into(), "not-for-renderer".into())]);
        let resolved = resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:codex-user:paint".into()] }, &environment).unwrap();
        assert_eq!(resolved.mcps["paint"]["headers"]["Authorization"], "Bearer not-for-renderer");
        assert!(!serde_json::to_string(&found).unwrap().contains("not-for-renderer"));
        assert!(resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:user:paint".into(), "mcp:codex-user:paint".into()] }, &environment).is_err());
    }

    #[test]
    fn a_workspace_outside_a_repository_never_reads_the_home_folder_as_its_project() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("work/project");
        std::fs::create_dir_all(&root).unwrap();
        write(&home.join(".codex/config.toml"), "[mcp_servers.paint]\ncommand = \"paint\"\n");
        write(&home.join(".claude/settings.json"), r#"{"enabledPlugins":{}}"#);
        write(&home.join("work/.codex/config.toml"), "[mcp_servers.parent]\ncommand = \"parent\"\n");
        let found = catalog(&home, &home.join(".claude"), Some(&root)).unwrap();
        assert!(found.entries.iter().find(|entry| entry.id == "mcp:codex-user:paint").unwrap().available, "the user's own server keeps its place");
        assert!(found.entries.iter().all(|entry| !entry.id.contains("project-parent")), "no folder above a non-repository workspace is a project folder");
        let resolved = resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:codex-user:paint".into()] }, &HashMap::new()).unwrap();
        assert_eq!(resolved.mcps["paint"]["command"], "paint");
        // Nor is $HOME itself, when it is the workspace.
        write(&home.join(".codex/skills/art/SKILL.md"), "---\nname: art\ndescription: Draw\n---\n");
        let at_home = catalog(&home, &home.join(".claude"), Some(&home)).unwrap();
        assert!(at_home.entries.iter().all(|entry| entry.group != "project" || entry.source == ".claude"), "{:?}", at_home.entries.iter().map(|entry| &entry.id).collect::<Vec<_>>());
        assert!(at_home.entries.iter().find(|entry| entry.id == "mcp:codex-user:paint").unwrap().available);
    }

    #[test]
    fn project_servers_need_claude_approval_and_show_only_their_program_or_host() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&root.join(".mcp.json"), &json!({"mcpServers":{
            "listed":{"command":"listed-cli","args":["--token=secret-arg"]},
            "local":{"type":"http","url":"https://user:secret-pass@mcp.example.test/path?key=secret-query","headers":{"Authorization":"secret-header"}},
            "refused":{"command":"refused-cli"},
            "committed":{"command":"committed-cli"},
            "same":{"command":"project-same"},
        }}).to_string());
        write(&home.join(".claude.json"), &json!({"mcpServers":{"same":{"command":"user-same"}},
            "projects":{root.to_string_lossy().as_ref():{"enabledMcpjsonServers":["listed"]}}}).to_string());
        write(&root.join(".claude/settings.local.json"), r#"{"enabledMcpjsonServers":["local","refused"],"disabledMcpjsonServers":["refused"]}"#);
        // The committed project settings cannot approve their own servers.
        write(&root.join(".claude/settings.json"), r#"{"enabledMcpjsonServers":["committed","same"],"enableAllProjectMcpServers":true}"#);
        let found = catalog(&home, &config, Some(&root)).unwrap();
        let entry = |id: &str| found.entries.iter().find(|entry| entry.id == id).unwrap();
        assert!(entry("mcp:project:listed").available);
        assert!(entry("mcp:project:local").available);
        assert!(!entry("mcp:project:refused").available);
        assert!(!entry("mcp:project:committed").available);
        assert!(!entry("mcp:project:same").available);
        assert!(entry("mcp:user:same").available, "an unapproved project server replaces nothing");
        assert_eq!(entry("mcp:project:listed").target.as_deref(), Some("listed-cli"));
        assert_eq!(entry("mcp:project:local").target.as_deref(), Some("mcp.example.test"));
        let public = serde_json::to_string(&found).unwrap();
        assert!(!public.contains("secret"), "{public}");
        assert!(resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:project:committed".into()] }, &HashMap::new()).is_err());
        write(&config.join("settings.json"), r#"{"enableAllProjectMcpServers":true}"#);
        let all = catalog(&home, &config, Some(&root)).unwrap();
        assert!(all.entries.iter().find(|entry| entry.id == "mcp:project:committed").unwrap().available);
        assert!(!all.entries.iter().find(|entry| entry.id == "mcp:project:refused").unwrap().available, "a refusal still wins");
    }
}
