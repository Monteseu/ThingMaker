//! Metadata-backed requirements and conservative, visible reference hints.
//! Detection never installs a server, invokes tools or exposes endpoint secrets.
use std::{collections::BTreeMap, path::Path, sync::LazyLock};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use super::ResourceEntry;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDependency {
    pub name: String,
    /// `declared` is authoritative metadata; `reference` is a suggestion only.
    pub evidence: String,
    pub required: bool,
    pub mcp_ids: Vec<String>,
}

struct Requirement {
    name: String,
    url: Option<String>,
    declared: bool,
    required: bool,
}

fn normalized(name: &str) -> String { name.chars().filter(char::is_ascii_alphanumeric).flat_map(char::to_lowercase).collect() }
fn identifier(name: &str) -> bool { !name.is_empty() && name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c)) }
fn name_matches(wanted: &str, configured: &str) -> bool {
    let wanted = normalized(wanted);
    let configured = normalized(configured);
    wanted == configured || configured.strip_suffix("mcp").is_some_and(|name| !name.is_empty() && name == wanted)
}
fn same_endpoint(left: &str, right: &str) -> bool {
    match (reqwest::Url::parse(left), reqwest::Url::parse(right)) {
        (Ok(mut left), Ok(mut right)) => {
            left.set_fragment(None); right.set_fragment(None);
            let left_path = left.path().trim_end_matches('/').to_string();
            let right_path = right.path().trim_end_matches('/').to_string();
            left.set_path(&left_path); right.set_path(&right_path);
            left == right
        }
        _ => left == right,
    }
}

fn metadata(value: &Value, requirements: &mut Vec<Requirement>, notes: &mut Vec<String>) {
    if let Some(tools) = value.pointer("/dependencies/tools") {
        if let Some(tools) = tools.as_array() {
            for tool in tools.iter().filter(|tool| tool["type"] == "mcp") {
                let name = tool["value"].as_str().filter(|name| identifier(name));
                let url = tool["url"].as_str().map(str::to_string);
                if name.is_some() || url.is_some() {
                    requirements.push(Requirement { name: name.unwrap_or("Declared MCP").into(), url, declared: true, required: tool["optional"] != true });
                } else { notes.push("An MCP dependency has no valid server identifier or endpoint.".into()); }
            }
        } else { notes.push("dependencies.tools must be a list; MCP requirements could not be fully read.".into()); }
    }
    // A compact project convention, also useful for Claude-authored skills.
    if let Some(servers) = value.get("mcp-servers") {
        if let Some(servers) = servers.as_array() {
            for server in servers {
                if let Some(name) = server.as_str().filter(|name| identifier(name)) {
                    requirements.push(Requirement { name: name.into(), url: None, declared: true, required: true });
                } else { notes.push("An mcp-servers entry is not a valid server identifier.".into()); }
            }
        } else { notes.push("mcp-servers must be a list of server identifiers.".into()); }
    }
}

static TOOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bmcp__([A-Za-z0-9_.:-]+(?:_[A-Za-z0-9_.:-]+)*)__[A-Za-z0-9_]+").unwrap());
static BEFORE_MCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b([A-Za-z][A-Za-z0-9_-]*)[ \t]+MCP\b").unwrap());
static AFTER_MCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\bMCP[ \t]+(?:(?:server|servidor|de|named)[ \t]+)?[`"']?([A-Za-z][A-Za-z0-9_-]*)"#).unwrap());
static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Za-z][A-Za-z0-9_-]*)://").unwrap());

fn references(text: &str, functions: Option<&Regex>, requirements: &mut Vec<Requirement>) {
    let mut add = |name: &str| requirements.push(Requirement { name: name.into(), url: None, declared: false, required: false });
    for capture in TOOL.captures_iter(text) { add(&capture[1]); }
    let ignored = ["the", "a", "an", "any", "all", "each", "this", "that", "its", "your", "our", "their", "el", "un", "los", "del", "de", "via", "through", "using", "with", "without", "only", "one", "own", "no", "new", "server", "servers", "servidor", "servidores", "tool", "tools", "connection", "connections", "connector", "connectors", "config", "configuration", "integration", "integrations", "is", "are", "was", "has", "can", "must", "not", "and", "or", "for", "from", "to", "available", "unavailable", "installed", "enabled", "disabled", "itself", "stdio", "http", "https", "sse", "json", "rpc"];
    for pattern in [&*BEFORE_MCP, &*AFTER_MCP] {
        for capture in pattern.captures_iter(text) {
            let name = &capture[1];
            if !ignored.contains(&name.to_ascii_lowercase().as_str()) { add(name); }
        }
    }
    // Resource URIs and concrete function calls also identify servers, even
    // when the skill never uses the word MCP (for example gitnexus://repos).
    for capture in SCHEME.captures_iter(text) {
        let name = &capture[1];
        if !["http", "https", "file", "mailto", "ssh", "git", "data", "plugin", "app", "codex", "skill", "vscode", "chrome", "chrome-extension"].contains(&name.to_ascii_lowercase().as_str()) { add(name); }
    }
    if let Some(functions) = functions { for capture in functions.captures_iter(text) { add(&capture[1]); } }
}

fn detect(file: &Path, entries: &[ResourceEntry], functions: Option<&Regex>) -> (Vec<McpDependency>, Vec<String>, String) {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file = canonical.as_path();
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut requirements = Vec::new();
    let mut notes = Vec::new();
    let mut revision = String::new();
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let front = text.lines().skip(1).take_while(|line| line.trim() != "---").collect::<Vec<_>>().join("\n");
        match serde_yaml_ng::from_str::<Value>(&front) {
            Ok(value) => metadata(&value, &mut requirements, &mut notes),
            Err(_) => notes.push("SKILL.md frontmatter could not be parsed for MCP dependencies; explicit references are still scanned.".into()),
        }
    }
    if let Some(directory) = file.parent() {
        for relative in ["SKILL.json", "agents/openai.yaml", "agents/openai.yml"] {
            let path = directory.join(relative);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    revision.push_str(&super::content_hash(&bytes));
                    let parsed = if relative.ends_with("json") { serde_json::from_slice::<Value>(&bytes).map_err(|_| ()) }
                        else { serde_yaml_ng::from_slice::<Value>(&bytes).map_err(|_| ()) };
                    match parsed {
                        Ok(value) => metadata(&value, &mut requirements, &mut notes),
                        Err(_) => notes.push(format!("{relative} could not be parsed for MCP dependencies.")),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => notes.push(format!("{relative} could not be read for MCP dependencies.")),
            }
        }
    }
    references(&text, functions, &mut requirements);
    let mut merged: BTreeMap<(String, Vec<String>), McpDependency> = BTreeMap::new();
    for requirement in requirements {
        let ids: Vec<_> = entries.iter().filter(|entry| entry.kind == "mcp" && match &requirement.url {
            Some(url) => entry.config["url"].as_str().is_some_and(|configured| same_endpoint(url, configured)),
            None => name_matches(&requirement.name, &entry.runtime_name) || name_matches(&requirement.name, entry.runtime_name.rsplit(':').next().unwrap_or(&entry.runtime_name)),
        }).map(|entry| entry.id.clone()).collect();
        let name = if requirement.name == "Declared MCP" && ids.len() == 1 { entries.iter().find(|entry| entry.id == ids[0]).unwrap().name.clone() } else { requirement.name };
        let key = (normalized(&name), ids.clone());
        let dependency = merged.entry(key).or_insert_with(|| McpDependency { name, evidence: "reference".into(), required: false, mcp_ids: ids });
        if requirement.declared { dependency.evidence = "declared".into(); }
        dependency.required |= requirement.required;
    }
    (merged.into_values().collect(), notes, super::content_hash(revision.as_bytes()))
}

pub(super) fn annotate(entries: &mut [ResourceEntry]) {
    let names = entries.iter().filter(|entry| entry.kind == "mcp").map(|entry| regex::escape(entry.runtime_name.rsplit(':').next().unwrap_or(&entry.runtime_name))).collect::<Vec<_>>();
    let functions = (!names.is_empty()).then(|| Regex::new(&format!(r"(?i)\b({})[_\.][A-Za-z][A-Za-z0-9_]*\s*\(", names.join("|"))).ok()).flatten();
    let detected: Vec<_> = entries.iter().map(|entry| entry.skill_path.as_ref().map(|file| detect(file, entries, functions.as_ref()))).collect();
    for (entry, detected) in entries.iter_mut().zip(detected) {
        if let Some((dependencies, notes, revision)) = detected {
            entry.mcp_dependencies = dependencies;
            entry.dependency_notes = notes;
            entry.revision.push_str(&format!(":{revision}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ResourceSelection, catalog, resolve};
    use std::collections::HashMap;
    use serde_json::json;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn skill<'a>(catalog: &'a super::super::ResourceCatalog, id: &str) -> &'a ResourceEntry {
        catalog.entries.iter().find(|entry| entry.id == id).unwrap()
    }

    #[test]
    fn metadata_and_references_match_installed_origins_without_exposing_endpoints() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let directory = home.join(".agents/skills/art");
        write(&directory.join("SKILL.md"), "---\nname: art\ndescription: Paint\nmcp-servers: [figma]\n---\nUse mcp__gitnexus__query and GitNexus MCP. Read gitnexus://repos. The MCP server is available.\n");
        write(&directory.join("agents/openai.yaml"), "dependencies:\n  tools:\n    - type: mcp\n      value: figma\n      url: https://figma.test/mcp?token=private-marker\n    - type: mcp\n      value: optional-service\n      optional: true\n    - type: env_var\n      value: PRIVATE_KEY\n");
        write(&directory.join("SKILL.json"), r#"{"dependencies":{"tools":[{"type":"mcp","value":"gitnexus"}]}}"#);
        write(&home.join(".claude.json"), &json!({"mcpServers":{"figma":{"url":"https://figma.test/mcp?token=private-marker"},"gitnexus":{"command":"gitnexus"}}}).to_string());
        let found = catalog(home, &home.join(".claude"), None).unwrap();
        let entry = skill(&found, "skill:agents-user:art");
        assert_eq!(entry.mcp_dependencies.len(), 3, "generic MCP prose must not create bogus server names");
        let figma = entry.mcp_dependencies.iter().find(|dependency| dependency.name == "figma").unwrap();
        assert!(figma.required && figma.evidence == "declared");
        assert_eq!(figma.mcp_ids, ["mcp:user:figma"]);
        assert!(!entry.mcp_dependencies.iter().find(|dependency| dependency.name == "optional-service").unwrap().required);
        assert!(!serde_json::to_string(&found).unwrap().contains("private-marker"));
        let chosen = ResourceSelection { skills: vec![entry.id.clone()], mcps: vec![] };
        assert!(resolve(&found, &chosen, &HashMap::new()).err().unwrap().message.contains("requires MCP"));
        let complete = ResourceSelection { mcps: vec!["mcp:user:figma".into(), "mcp:user:gitnexus".into()], ..chosen };
        assert!(resolve(&found, &complete, &HashMap::new()).is_ok());
    }

    #[test]
    fn missing_dependencies_disabled_candidates_and_provider_compatibility_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&home.join(".agents/skills/art/SKILL.md"), "---\nname: art\ndescription: Draw\n---\nUse mcp__paint__draw, mcp__events__read and mcp__missing__read.\n");
        write(&home.join(".claude.json"), r#"{"mcpServers":{"paint":{"type":"sse","url":"https://paint.test/events"},"events":{"type":"sse","url":"https://events.test/sse"}}}"#);
        write(&root.join(".mcp.json"), r#"{"mcpServers":{"paint":{"command":"paint-cli"}}}"#);
        write(&root.join(".claude/settings.local.json"), r#"{"enabledMcpjsonServers":["paint"]}"#);
        write(&home.join(".codex/config.toml"), "[mcp_servers.paint]\ncommand=\"other-paint\"\nenabled=false\n");
        let found = catalog(home, &home.join(".claude"), Some(&root)).unwrap();
        let entry = skill(&found, "skill:agents-user:art");
        let paint = entry.mcp_dependencies.iter().find(|dependency| dependency.name == "paint").unwrap();
        assert_eq!(paint.mcp_ids.len(), 3, "all origins remain visible, including disabled definitions");
        assert!(entry.mcp_dependencies.iter().find(|dependency| dependency.name == "missing").unwrap().mcp_ids.is_empty());
        assert!(!skill(&found, "mcp:user:paint").available, "project has native precedence");
        assert!(!skill(&found, "mcp:codex-user:paint").available);
        assert!(skill(&found, "mcp:user:events").available);
        let codex = found.for_provider(crate::agents::Provider::Codex);
        assert!(!skill(&codex, "mcp:user:paint").available);
        assert!(skill(&codex, "mcp:project:paint").available);
        assert!(!skill(&codex, "mcp:user:events").available);
    }

    #[test]
    fn metadata_changes_invalidate_sessions_and_bad_metadata_is_reported_without_guessing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let directory = home.join(".codex/skills/art");
        write(&directory.join("SKILL.md"), "---\nname: art\ndescription: Draw\n---\nUse mcp__paint__draw.\n");
        let selected = ResourceSelection { skills: vec!["skill:codex-user:art".into()], mcps: vec![] };
        let first = catalog(home, &home.join(".claude"), None).unwrap();
        let before = resolve(&first, &selected, &HashMap::new()).unwrap();
        write(&directory.join("agents/openai.yaml"), "dependencies:\n  tools:\n    - type: mcp\n      value: paint\n");
        let declared = catalog(home, &home.join(".claude"), None).unwrap();
        assert!(resolve(&declared, &selected, &HashMap::new()).is_err());
        write(&directory.join("agents/openai.yaml"), "dependencies:\n  tools:\n    - type: mcp\n      value: paint\n      optional: true\n");
        let optional = catalog(home, &home.join(".claude"), None).unwrap();
        assert_ne!(before.revision, resolve(&optional, &selected, &HashMap::new()).unwrap().revision);
        write(&directory.join("agents/openai.yaml"), "dependencies: [malformed\n");
        let malformed = catalog(home, &home.join(".claude"), None).unwrap();
        let entry = skill(&malformed, &selected.skills[0]);
        assert!(!entry.dependency_notes.is_empty());
        assert_eq!(entry.mcp_dependencies[0].evidence, "reference");
        assert!(!entry.mcp_dependencies[0].required);
    }
}
