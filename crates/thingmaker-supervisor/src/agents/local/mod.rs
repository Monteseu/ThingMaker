//! Local Models: models installed in Ollama on this computer.
//!
//! Two ways to run one. **Chat** is [`chat_agent`], a small ACP agent inside
//! this executable that passes messages to Ollama and has no tools.
//! **Agent** is Claude Code's harness pointed at Ollama, in a sandbox that
//! lets it reach only this computer (see `docs/plans/local-models.md`).
//!
//! A Local session is a Claude-style launch carrying a [`LocalTarget`]: the
//! session actor, transcript and screens are shared, and what differs is the
//! program started, its environment and its sandbox.

#[cfg(windows)]
pub mod appcontainer;
pub mod chat_agent;
pub mod ollama;
pub mod recommended;
pub mod sandbox;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a local model runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalKind {
    /// Talks; no tools, no agent program.
    Chat,
    /// Claude Code's agent with the local model as its brain.
    Agent,
}

/// A Local model id as stored on a session or a worker: the kind, then
/// Ollama's own name. `@` cannot start an Ollama name, so the two never mix.
const CHAT_PREFIX: &str = "@chat/";
const AGENT_PREFIX: &str = "@agent/";

/// The context window a session asks for when nothing says otherwise. 16k
/// fits Claude Code's ~4k-token start with room, and an 8B model's memory
/// stays near 7 GB on a 16 GB computer (Phase 0).
pub const DEFAULT_WINDOW: u64 = 16_384;

/// Splits a stored model id into its kind and Ollama name. A bare name is a
/// chat: that is what a person most often wants from a local model.
pub fn parse_model(id: &str) -> (LocalKind, &str) {
    if let Some(name) = id.strip_prefix(AGENT_PREFIX) {
        (LocalKind::Agent, name)
    } else {
        (LocalKind::Chat, id.strip_prefix(CHAT_PREFIX).unwrap_or(id))
    }
}

pub fn model_id(kind: LocalKind, name: &str) -> String {
    match kind {
        LocalKind::Chat => format!("{CHAT_PREFIX}{name}"),
        LocalKind::Agent => format!("{AGENT_PREFIX}{name}"),
    }
}

/// Everything a Local launch adds to a Claude-style one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTarget {
    pub kind: LocalKind,
    /// Ollama's name for the model.
    pub model: String,
    /// The name requests use: for an agent, the copy that carries the
    /// window (`qwen3:8b-thingmaker-16k`); for a chat, the model itself.
    pub served: String,
    pub window: u64,
    /// ThingMaker's folder for Local Models: chat conversations under
    /// `chats/`, the agent's own Claude Code settings under `claude/`.
    pub home: PathBuf,
}

impl LocalTarget {
    pub fn chat_dir(home: &Path) -> PathBuf {
        home.join("chats")
    }

    /// The agent's Claude Code folder. The person's `~/.claude` (login, MCP
    /// servers, plugins) is never read by a local session.
    pub fn claude_dir(home: &Path) -> PathBuf {
        home.join("claude")
    }

    /// The variables the started program gets, set last so nothing
    /// inherited (an `ANTHROPIC_*` in the person's runtime environment, say)
    /// can override them.
    pub fn environment(&self) -> Vec<(String, String)> {
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        match self.kind {
            LocalKind::Chat => vec![
                pair(chat_agent::MODEL_ENV, &self.served),
                pair(chat_agent::DIR_ENV, &Self::chat_dir(&self.home).to_string_lossy()),
                pair(chat_agent::WINDOW_ENV, &self.window.to_string()),
            ],
            LocalKind::Agent => {
                let mut env = vec![
                    pair("ANTHROPIC_BASE_URL", &ollama::address()),
                    pair("ANTHROPIC_AUTH_TOKEN", "ollama"),
                    pair("ANTHROPIC_API_KEY", ""),
                    pair("CLAUDE_CONFIG_DIR", &Self::claude_dir(&self.home).to_string_lossy()),
                    pair("CLAUDE_CODE_MAX_CONTEXT_TOKENS", &self.window.to_string()),
                    pair("CLAUDE_CODE_AUTO_COMPACT_WINDOW", &self.window.to_string()),
                    // Claude Code keeps room for its reply out of the window;
                    // its default (tens of thousands of tokens) would leave
                    // no room at all for the prompt in a local window.
                    pair("CLAUDE_CODE_MAX_OUTPUT_TOKENS", &(self.window / 4).to_string()),
                ];
                // In an AppContainer the person's temporary folder is out of
                // reach; the agent gets one inside its own folder.
                if cfg!(windows) {
                    let temp = self.home.join("tmp").to_string_lossy().into_owned();
                    env.push(pair("TEMP", &temp));
                    env.push(pair("TMP", &temp));
                }
                // Every model Claude Code would pick by itself (titles,
                // compaction, subagents) is this one, not "haiku".
                for name in ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL"] {
                    env.push(pair(name, &self.served));
                }
                // No telemetry, error reports, feature flags or update
                // checks: with these off, Phase 0 saw only 127.0.0.1.
                for name in ["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING", "DISABLE_AUTOUPDATER", "DISABLE_BUG_COMMAND", "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"] {
                    env.push(pair(name, "1"));
                }
                // A lean start. In a real project Claude Code's start is tens
                // of thousands of tokens (a large game project: 42,733, mostly its
                // CLAUDE.md, rules and skills), more than a local window holds
                // and minutes to read on a laptop. With these it is about
                // 2,500 and tools still work. The project's own instructions
                // are not read: they would not fit.
                for name in ["CLAUDE_CODE_SIMPLE", "CLAUDE_CODE_DISABLE_CLAUDE_MDS", "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS", "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS", "CLAUDE_CODE_DISABLE_AUTO_MEMORY"] {
                    env.push(pair(name, "1"));
                }
                env
            }
        }
    }

    /// Tools an agent is never offered: they reach the internet, which the
    /// sandbox refuses anyway, so offering them only wastes the model's turn.
    pub fn disallowed_tools(&self) -> Vec<String> {
        match self.kind {
            LocalKind::Chat => Vec::new(),
            LocalKind::Agent => vec!["WebFetch".into(), "WebSearch".into()],
        }
    }
}

/// ThingMaker's folder for Local Models inside its data folder.
pub fn local_home(data_dir: &Path) -> PathBuf {
    data_dir.join("local-models")
}

/// Ollama's program: on PATH, in the usual install folders, or inside the
/// Ollama app. A desktop app opened from the Dock has no shell PATH.
pub fn find_ollama(home: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "ollama.exe" } else { "ollama" };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = std::env::split_paths(&path_var).map(|dir| dir.join(name)).collect();
    candidates.extend([
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/Applications/Ollama.app/Contents/Resources").join(name),
        home.join("Applications/Ollama.app/Contents/Resources").join(name),
        home.join("AppData/Local/Programs/Ollama").join(name),
    ]);
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Whether a local session's conversation exists, for a resume: a chat's
/// file, or the agent's Claude Code transcript.
pub fn conversation_exists(home: &Path, root: &Path, session_id: &str) -> bool {
    chat_file(home, session_id).is_some_and(|path| path.is_file())
        || crate::agents::claude::transcript_usage::transcript_path_in(&LocalTarget::claude_dir(home), root, session_id).is_file()
}

fn chat_file(home: &Path, session_id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(session_id).ok().map(|_| LocalTarget::chat_dir(home).join(format!("{session_id}.jsonl")))
}

/// The models a picker offers: every installed model as a chat, and the
/// ones that use tools as an agent too. A model that thinks gets Think
/// (off/on) as its effort, in a chat.
pub fn catalog(models: &[ollama::LocalModel]) -> Vec<crate::agents::auth::ProviderModel> {
    let mut out = Vec::new();
    for model in models {
        let size = if model.size_bytes > 0 { format!(" · {:.1} GB", model.size_bytes as f64 / 1e9) } else { String::new() };
        let context = model.context_length.map(|tokens| format!(" · {}k context", tokens / 1024)).unwrap_or_default();
        let facts = format!("{}{}{size}{context}", model.parameter_size, if model.quantization.is_empty() { String::new() } else { format!(" · {}", model.quantization) });
        let thinks = model.capabilities.iter().any(|capability| capability == "thinking");
        out.push(crate::agents::auth::ProviderModel {
            id: model_id(LocalKind::Chat, &model.name),
            name: model.name.clone(),
            description: Some(format!("Chat · {facts}")),
            needs_credits: false,
            efforts: if thinks { vec!["off".into(), "on".into()] } else { Vec::new() },
            default_effort: thinks.then(|| "off".into()),
            input_image: model.sees_images(),
            is_default: false,
        });
        if model.uses_tools() {
            out.push(crate::agents::auth::ProviderModel {
                id: model_id(LocalKind::Agent, &model.name),
                name: model.name.clone(),
                description: Some(format!("Agent · {facts}")),
                needs_credits: false,
                efforts: Vec::new(),
                default_effort: None,
                input_image: model.sees_images(),
                is_default: false,
            });
        }
    }
    if let Some(first) = out.first_mut() {
        first.is_default = true;
    }
    out
}

/// The model name a local agent's requests use: the copy of `model` that
/// carries a `window`-token window, made if missing. Ollama's client blocks,
/// so this runs on its own thread and is safe to call from async code.
pub fn prepare_agent_model(model: &str, window: u64) -> Result<String, String> {
    let model = model.to_string();
    std::thread::spawn(move || ollama::Ollama::local().with_window(&model, window)).join().map_err(|_| "Ollama's client stopped unexpectedly".to_string())?
}

/// The first installed model, for a session that names none: as a chat,
/// which every model can be. Safe to call from async code.
pub fn first_installed() -> Option<String> {
    std::thread::spawn(|| ollama::Ollama::local().models().ok()?.into_iter().next().map(|model| model_id(LocalKind::Chat, &model.name)))
        .join()
        .ok()
        .flatten()
}

/// This computer's memory, for whether a model fits.
pub fn memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_carry_the_kind_and_a_bare_name_is_a_chat() {
        assert_eq!(parse_model("@agent/qwen3:8b"), (LocalKind::Agent, "qwen3:8b"));
        assert_eq!(parse_model("@chat/llama3.1:8b"), (LocalKind::Chat, "llama3.1:8b"));
        assert_eq!(parse_model("hf.co/org/model:Q4"), (LocalKind::Chat, "hf.co/org/model:Q4"));
        assert_eq!(model_id(LocalKind::Agent, "qwen3:8b"), "@agent/qwen3:8b");
    }

    #[test]
    fn an_agent_is_pointed_at_ollama_with_nothing_of_the_persons_claude() {
        let target = LocalTarget { kind: LocalKind::Agent, model: "qwen3:8b".into(), served: "qwen3:8b-thingmaker-16k".into(), window: 16384, home: "/data/local-models".into() };
        let env: std::collections::HashMap<_, _> = target.environment().into_iter().collect();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:11434");
        assert_eq!(env["ANTHROPIC_API_KEY"], "");
        assert_eq!(env["CLAUDE_CONFIG_DIR"], "/data/local-models/claude");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "qwen3:8b-thingmaker-16k");
        assert_eq!(env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], "1");
        assert_eq!(env["CLAUDE_CODE_SIMPLE"], "1", "a lean start that fits a local window");
        assert_eq!(env["CLAUDE_CODE_DISABLE_CLAUDE_MDS"], "1");
        assert!(!env.contains_key("CLAUDE_CODE_ARTIFACT"), "the Artifact tool is a cloud service");
        assert_eq!(target.disallowed_tools(), ["WebFetch", "WebSearch"]);
        let mut options = crate::agents::claude::launch::ClaudeLaunchOptions::new("/w", "/usr/bin/node", vec!["adapter.js".into()]);
        options.append_system_prompt = Some("Lead the team.".into());
        options.local = Some(target.clone());
        let meta = options.session_meta().unwrap();
        assert_eq!(meta["systemPrompt"]["append"], serde_json::json!("Lead the team.\n\n/no_think"), "the model is asked not to think, after any guidance");
        assert_eq!(meta["claudeCode"]["options"]["disallowedTools"], serde_json::json!(["WebFetch", "WebSearch"]));
        assert_eq!(meta["claudeCode"]["options"]["settingSources"], serde_json::json!(["user"]), "the project's own settings are not read");
        assert_eq!(meta["claudeCode"]["options"]["thinking"], serde_json::json!({ "type": "disabled" }));
        assert_eq!(meta["claudeCode"]["options"]["promptSuggestions"], serde_json::json!(false));

        let chat = LocalTarget { kind: LocalKind::Chat, served: "qwen3:8b".into(), ..target };
        let env: std::collections::HashMap<_, _> = chat.environment().into_iter().collect();
        assert_eq!(env[chat_agent::MODEL_ENV], "qwen3:8b");
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn every_model_can_chat_and_only_those_with_tools_are_agents() {
        let model = |name: &str, capabilities: &[&str]| ollama::LocalModel { name: name.into(), family: String::new(), parameter_size: "8B".into(), quantization: "Q4_K_M".into(), size_bytes: 5_200_000_000, context_length: Some(40960), capabilities: capabilities.iter().map(|c| c.to_string()).collect() };
        let list = catalog(&[model("gemma3:4b", &["completion", "vision"]), model("qwen3:8b", &["completion", "tools", "thinking"])]);
        let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["@chat/gemma3:4b", "@chat/qwen3:8b", "@agent/qwen3:8b"]);
        assert!(list[0].input_image);
        assert_eq!(list[1].efforts, ["off", "on"], "a model that thinks gets Think in a chat");
        assert!(list[2].efforts.is_empty());
        assert_eq!(list[1].description.as_deref(), Some("Chat · 8B · Q4_K_M · 5.2 GB · 40k context"));
    }
}
