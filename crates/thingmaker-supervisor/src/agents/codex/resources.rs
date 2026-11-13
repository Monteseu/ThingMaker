//! Session-only resource overrides. Never use skills/config/write: that API
//! persists to the person's config, while every worker owns its own app-server.
use std::{collections::BTreeSet, path::Path};
use serde_json::{Map, Value, json};
use super::launch::CodexResources;

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
}
