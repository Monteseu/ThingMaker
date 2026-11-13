//! Session-only resource overrides. Never use skills/config/write: that API
//! persists to the person's config, while every worker owns its own app-server.
use std::{collections::BTreeSet, path::Path};
use serde_json::{Map, Value, json};
use super::launch::CodexResources;

/// Codex accepts only `^[a-zA-Z0-9_-]+$` as a server name, and every selected
/// server is required, so a name like `plugin:art:paint` would stop the
/// worker. Other names become their valid characters plus a hash of the
/// original: stable across launches, and never the same as another server's.
pub fn server_names(mcps: std::collections::BTreeMap<String, Value>) -> std::collections::BTreeMap<String, Value> {
    let valid = |name: &str| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let (mut out, renamed): (std::collections::BTreeMap<_, _>, std::collections::BTreeMap<_, _>) = mcps.into_iter().partition(|(name, _)| valid(name));
    for (name, server) in renamed {
        let cleaned: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let hash = crate::storage::content_hash(name.as_bytes());
        let mut length = 8;
        let mut codex = format!("{}_{}", cleaned.trim_matches('_'), &hash[..length]);
        while out.contains_key(&codex) && length < hash.len() {
            length += 8;
            codex = format!("{}_{}", cleaned.trim_matches('_'), &hash[..length]);
        }
        out.insert(codex, server);
    }
    out
}

pub fn overrides(effective: &Value, discovered: &Value, selected: &CodexResources) -> Result<Map<String, Value>, String> {
    let rows = discovered["data"].as_array().ok_or("Codex did not provide a skills inventory")?;
    let mut rules = Vec::new();
    let mut names = BTreeSet::new();
    let mut matched = BTreeSet::new();
    let mut enable = Vec::new();
    for row in rows {
        // Malformed unselected skills do not load. A malformed selected skill
        // fails the exact-path match below; the visible catalog stays frozen.
        for skill in row["skills"].as_array().ok_or("Codex returned an invalid skills inventory")? {
            let name = skill["name"].as_str().ok_or("Codex returned a skill without a name")?;
            let path = skill["path"].as_str().ok_or("Codex returned a skill without a path")?;
            if names.insert(name.to_string()) { rules.push(json!({"name":name,"enabled":false})); }
            rules.push(json!({"path":path,"enabled":false}));
            let canonical = Path::new(path).canonicalize().ok();
            for wanted in &selected.skills {
                if canonical.as_ref() == Some(wanted) {
                    matched.insert(wanted.clone());
                    enable.push(json!({"path":path,"enabled":true}));
                }
            }
        }
    }
    if selected.skills.iter().any(|path| !matched.contains(path)) {
        return Err("A selected skill could not be discovered by Codex. Check its frontmatter and name collisions.".into());
    }
    rules.extend(enable);
    let effective = effective.get("config").ok_or("Codex did not provide its effective configuration")?;
    let mut mcps = Map::new();
    // A root-table override replaces the inherited table. Supply only complete
    // selected definitions; disabled skeletons have no valid transport.
    for (name, server) in &selected.mcps { mcps.insert(name.clone(), server_config(server)?); }
    let mut plugins = Map::new();
    for (name, _) in effective["plugins"].as_object().into_iter().flatten() { plugins.insert(name.clone(), json!({"enabled":false})); }
    Ok(Map::from_iter([
        ("skills.config".into(), Value::Array(rules)),
        ("skills.bundled.enabled".into(), json!(false)),
        // Freeze the model-visible catalog at launch. A newly installed native
        // skill must not enter an already-running restricted worker's context.
        ("skills.include_instructions".into(), json!(false)),
        ("features.apps".into(), json!(false)),
        ("features.multi_agent".into(), json!(false)),
        ("agents.enabled".into(), json!(false)),
        ("mcp_servers".into(), Value::Object(mcps)),
        ("plugins".into(), Value::Object(plugins)),
    ]))
}

fn server_config(server: &Value) -> Result<Value, String> {
    let mut out = Map::from_iter([("enabled".into(), json!(true)), ("required".into(), json!(true))]);
    if server["command"].is_string() {
        for key in ["command", "args", "env", "cwd"] { if let Some(value) = server.get(key) { out.insert(key.into(), value.clone()); } }
    } else if server["url"].is_string() && server["type"] != "sse" {
        out.insert("url".into(), server["url"].clone());
        if let Some(headers) = server.get("headers") { out.insert("http_headers".into(), headers.clone()); }
    } else { return Err("A selected MCP transport is not supported by Codex".into()) }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_resources_are_disabled_and_only_selected_paths_and_servers_are_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        std::fs::write(&path, "test").unwrap();
        let path = path.canonicalize().unwrap();
        let selected = CodexResources { skills: vec![path.clone()], mcps: std::collections::BTreeMap::from([("art".into(), json!({"command":"paint"}))]) };
        let effective = json!({"config":{"mcp_servers":{"qa":{"command":"qa"}},"plugins":{"qa@market":{"enabled":true}}}});
        let discovered = json!({"data":[{"errors":[],"skills":[{"name":"art","path":path}]}]});
        let config = overrides(&effective, &discovered, &selected).unwrap();
        assert!(config["mcp_servers"].get("qa").is_none());
        assert_eq!(config["mcp_servers"]["art"]["required"], true);
        assert_eq!(config["plugins"]["qa@market"]["enabled"], false);
        assert_eq!(config["skills.config"].as_array().unwrap().last().unwrap()["enabled"], true);
        assert_eq!(config["skills.include_instructions"], false);
    }

    #[test]
    fn server_names_codex_refuses_are_renamed_stably_and_without_collisions() {
        let valid = |name: &str| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let selected = std::collections::BTreeMap::from([
            ("plugin:art:paint".to_string(), json!({"command":"paint"})),
            ("plugin_art_paint".to_string(), json!({"command":"other"})),
            ("plugin:art.paint".to_string(), json!({"command":"third"})),
            ("files".to_string(), json!({"command":"files"})),
        ]);
        let named = server_names(selected.clone());
        assert_eq!(named.len(), 4);
        assert!(named.keys().all(|name| valid(name)), "{named:?}");
        assert_eq!(named["files"]["command"], "files");
        assert_eq!(named["plugin_art_paint"]["command"], "other", "a valid name is kept");
        let renamed: Vec<_> = named.iter().filter(|(name, _)| name.starts_with("plugin_art_paint_")).map(|(_, server)| server["command"].clone()).collect();
        assert_eq!(renamed.len(), 2);
        assert_eq!(server_names(selected), named, "the same selection gets the same names");
    }
}
