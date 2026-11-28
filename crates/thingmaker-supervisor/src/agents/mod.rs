//! The agents the desktop can attach to, and everything about them that is
//! not ordinary ACP.
//!
//! The session actor is provider-agnostic — ACP over stdio, one turn at a
//! time — and the places where providers genuinely differ are gathered here:
//! the command line, how the session id is chosen, what `session/new` carries,
//! how subagents and quota appear, and where the transcript lives. A reader
//! who wants to know what "Claude-specific" or "Codex-specific" means should
//! find the whole answer in this module.
//!
//! Adding a provider is one variant of [`Provider`], one of [`AgentLaunch`]
//! and one submodule. An agent CLI that already speaks ACP needs nothing
//! else; one that speaks its own protocol gets a bridge that translates it
//! into ACP inside the supervisor, the way `codex` does.

pub mod accounts;
pub mod auth;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod events;
pub mod gemini;
pub mod local;
pub mod login;
pub mod usage;
pub mod titles;
pub mod versions;

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::{claude::launch::ClaudeLaunchOptions, codex::launch::CodexLaunchOptions, gemini::launch::GeminiLaunchOptions, copilot::CopilotLaunchOptions};

/// What an attachment may do without asking, decided once from the
/// workspace's trust state (§2.2).
///
/// An agent runs unattended for hours, so a permission request nobody answers
/// would end it at its first edit. Each provider maps the stance onto its own
/// modes: Claude Code's `acceptEdits`/`plan`, Codex's sandbox and approval
/// policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionStance {
    /// Edits are accepted without asking; anything else that still arrives is
    /// allowed and journalled. For a `trusted_local` workspace, which is the
    /// same gate a desktop-run check passes.
    AcceptEdits,
    /// The agent plans rather than acts, and what still arrives is refused.
    /// For `inspect_only`.
    ReadOnly,
}

impl PermissionStance {
    /// Whether a permission request that still arrives is allowed. Edits are
    /// pre-answered by the mode; this is for the rest — shell, network, a
    /// tool the mode does not cover.
    pub fn allows(self) -> bool {
        matches!(self, Self::AcceptEdits)
    }
}

/// The subscription an attachment runs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Claude,
    Codex,
    /// Antigravity's `agy`: a worker or a plain session on its stream-json
    /// mode, and an orchestrator on native `agy acp`, the only mode that
    /// takes a session's own MCP server.
    Gemini,
    /// GitHub Copilot CLI on native ACP.
    Copilot,
    /// Models in Ollama on this computer: free, and private.
    Local,
}

impl Provider {
    pub const ALL: [Provider; 5] = [Provider::Claude, Provider::Codex, Provider::Gemini, Provider::Copilot, Provider::Local];

    /// Whether a session on this provider can lead a team: it has to take
    /// the session's own `team` MCP server. Gemini takes one only over
    /// native `agy acp`, which is what a Gemini session that leads a team is
    /// opened on.
    pub fn can_orchestrate(self) -> bool {
        true
    }

    /// Whether a worker on this provider is handed its own reduced `team`
    /// server. A Gemini worker is not: it stays on the stream-json bridge,
    /// which cannot forward one, and takes part in the agent chat through
    /// its replies instead (`TEAM_CHAT` lines). The two are never both given.
    pub fn worker_team_server(self) -> bool {
        !matches!(self, Self::Gemini)
    }

    /// Big Thing's policy/schema remains separate from ordinary team leadership.
    pub fn can_lead_goal(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Local)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Local => "local",
        }
    }

    /// Reads a stored value. Unknown values are an error rather than a
    /// default: a session recorded under a provider this build does not have
    /// cannot be opened, and pretending otherwise would launch the wrong one.
    pub fn parse(value: &str) -> Option<Provider> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "copilot" => Some(Self::Copilot),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// What to call it on screen and in an error a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::Copilot => "GitHub Copilot",
            Self::Local => "Local Models",
        }
    }
}

/// Turns Claude Code's Artifact tool on in an SDK session, where it is off
/// by default.
pub const CLAUDE_ARTIFACT_ENV: &str = "CLAUDE_CODE_ARTIFACT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLaunch {
    Claude(ClaudeLaunchOptions),
    Codex(CodexLaunchOptions),
    Gemini(GeminiLaunchOptions),
    Copilot(CopilotLaunchOptions),
}

impl AgentLaunch {
    pub fn provider(&self) -> Provider {
        match self {
            Self::Claude(options) if options.local.is_some() => Provider::Local,
            Self::Claude(_) => Provider::Claude,
            Self::Codex(options) if options.local => Provider::Local,
            Self::Codex(_) => Provider::Codex,
            Self::Gemini(_) => Provider::Gemini,
            Self::Copilot(_) => Provider::Copilot,
        }
    }

    pub fn root(&self) -> &Path {
        match self {
            Self::Claude(options) => &options.root,
            Self::Codex(options) => &options.root,
            Self::Gemini(options) => &options.root,
            Self::Copilot(options) => &options.root,
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        match self {
            Self::Claude(options) => options.arguments(),
            Self::Codex(options) => options.arguments(),
            Self::Gemini(options) => options.arguments(),
            Self::Copilot(options) => options.arguments(),
        }
    }

    /// The id this attachment is addressed by inside the desktop.
    ///
    /// Agents choose their own session ids and say so in the `session/new`
    /// reply, so on a new session this is a reservation the agent will
    /// replace, and on a resume it is the agent's own id handed back to it.
    /// Pure either way: the reservation is made once, when the options are
    /// built, because the handle and the duplicate-attachment check both read
    /// this and a fresh id per call would let them disagree.
    pub fn session_id(&self) -> String {
        match self {
            Self::Claude(options) => options.resume.clone().unwrap_or_else(|| options.reserved.to_string()),
            Self::Codex(options) => options.resume.clone().unwrap_or_else(|| options.reserved.to_string()),
            Self::Gemini(options) => options.resume.clone().unwrap_or_else(|| options.reserved.to_string()),
            Self::Copilot(options) => options.resume.clone().unwrap_or_else(|| options.reserved.to_string()),
        }
    }

    /// Whether this attachment resumes a session the agent already has.
    pub fn is_resume(&self) -> bool {
        match self {
            Self::Claude(options) => options.resume.is_some(),
            Self::Codex(options) => options.resume.is_some(),
            Self::Gemini(options) => options.resume.is_some(),
            Self::Copilot(options) => options.resume.is_some(),
        }
    }

    /// The permission stance this attachment runs under.
    pub fn permission(&self) -> PermissionStance {
        match self {
            Self::Claude(options) => options.permission_mode,
            Self::Codex(options) => options.permission_mode,
            Self::Gemini(options) => options.permission_mode,
            Self::Copilot(options) => options.permission_mode,
        }
    }

    /// The `mode` config option value for [`AgentLaunch::permission`], by
    /// the agent's own id for it.
    pub fn permission_mode_id(&self) -> &'static str {
        match self {
            Self::Claude(options) => claude::launch::mode_id(options.permission_mode),
            Self::Codex(options) => codex::launch::sandbox_mode(options.permission_mode),
            Self::Gemini(options) => gemini::launch::mode_id(options.permission_mode),
            Self::Copilot(options) => copilot::launch::mode_id(options.permission_mode),
        }
    }

    /// The model to select once the session is open.
    pub fn deferred_model(&self) -> Option<&str> {
        match self {
            Self::Claude(options) => options.model.as_deref(),
            Self::Codex(options) => options.model.as_deref(),
            Self::Gemini(options) => options.model.as_deref(),
            Self::Copilot(options) => options.model.as_deref(),
        }
    }

    /// Where to land when `deferred_model` cannot be selected.
    pub fn deferred_model_fallback(&self) -> Option<&str> {
        match self {
            Self::Claude(options) => options.model_fallback.as_deref(),
            Self::Codex(_) | Self::Gemini(_) | Self::Copilot(_) => None,
        }
    }

    /// The reasoning effort to select once the session is open.
    pub fn deferred_effort(&self) -> Option<&str> {
        match self {
            Self::Claude(options) => options.effort.as_deref(),
            Self::Codex(options) => options.effort.as_deref(),
            Self::Gemini(options) => options.effort.as_deref(),
            Self::Copilot(options) => options.effort.as_deref(),
        }
    }

    /// `_meta` for `session/new` and `session/load`, when the provider takes
    /// options there that it has no config option for.
    pub fn session_meta(&self) -> Option<Value> {
        match self {
            Self::Claude(options) => options.session_meta(),
            Self::Codex(_) | Self::Gemini(_) | Self::Copilot(_) => None,
        }
    }

    /// Variables this provider's program needs set beyond the documented
    /// profile: Codex's `CODEX_HOME` for an account that is not the default,
    /// Claude's account folder and Artifact switch, and a Local Models
    /// session's own (see `agents::local`).
    pub fn environment(&self) -> Vec<(String, String)> {
        match self {
            Self::Claude(options) => match &options.local {
                Some(local) => {
                    let mut env = local.environment();
                    if options.resources.as_ref().is_some_and(|(skills, mcps)| !skills.is_empty() || !mcps.is_empty()) {
                        env.retain(|(key, _)| key != "CLAUDE_CODE_SIMPLE");
                        env.push(("CLAUDE_CODE_SIMPLE".into(), "0".into()));
                    }
                    env
                },
                None => {
                    let mut env: Vec<(String, String)> = options.config_dir.iter().map(|dir| ("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned())).collect();
                    // Claude Code withholds its Artifact tool from SDK
                    // sessions ("sdk_default_off") unless this is set. The
                    // person's own settings and their org's policy still
                    // decide after it.
                    env.push((CLAUDE_ARTIFACT_ENV.into(), "1".into()));
                    env
                },
            },
            Self::Gemini(options) => gemini::launch::account_environment(options.account_home.as_deref()),
            Self::Copilot(options) => options.environment(),
            Self::Codex(options) => {
                let mut env = Vec::new();
                if let Some(home) = &options.codex_home {
                    env.push(("CODEX_HOME".into(), home.to_string_lossy().into_owned()));
                    if cfg!(windows) && options.local && let Some(parent) = home.parent() {
                        let temp = parent.join("tmp").to_string_lossy().into_owned();
                        env.extend([("TEMP".into(), temp.clone()), ("TMP".into(), temp)]);
                    }
                }
                env
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::launch::SessionId;

    #[test]
    fn providers_round_trip_through_their_stored_names() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.as_str()), Some(provider));
        }
        assert_eq!(Provider::parse("kit"), None, "a Kit session cannot be opened by this build");
    }

    #[test]
    fn the_agent_chooses_the_session_id_unless_it_is_handed_back() {
        let fresh = AgentLaunch::Claude(ClaudeLaunchOptions::new("/work", "/usr/bin/node", vec!["adapter.js".into()]));
        assert!(!fresh.is_resume());
        // The reservation is only a placeholder, but it still has to be a
        // legal session id: it is the handle the desktop addresses. And it
        // has to be the *same* one every time it is asked, or the handle and
        // the duplicate-attachment check would name different sessions.
        assert!(SessionId::new(fresh.session_id()).is_ok());
        assert_eq!(fresh.session_id(), fresh.session_id());

        let mut options = ClaudeLaunchOptions::new("/work", "/usr/bin/node", vec!["adapter.js".into()]);
        options.resume = Some("d847b2a3-c7b5-4459-8b0f-02fdd03031a4".into());
        options.model = Some("sonnet".into());
        options.permission_mode = PermissionStance::AcceptEdits;
        let resumed = AgentLaunch::Claude(options);
        assert_eq!(resumed.session_id(), "d847b2a3-c7b5-4459-8b0f-02fdd03031a4");
        assert!(resumed.is_resume(), "a resume hands the id back");
        assert_eq!(resumed.deferred_model(), Some("sonnet"));
        assert_eq!(resumed.permission(), PermissionStance::AcceptEdits);
        assert_eq!(resumed.permission_mode_id(), "acceptEdits");
        assert_eq!(resumed.arguments(), ["adapter.js"]);
        assert_eq!(resumed.provider(), Provider::Claude);
        assert_eq!(resumed.root(), Path::new("/work"));
    }

    #[test]
    fn a_claude_session_turns_the_artifact_tool_on() {
        let mut options = ClaudeLaunchOptions::new("/work", "/usr/bin/node", vec!["adapter.js".into()]);
        let env: std::collections::HashMap<_, _> = AgentLaunch::Claude(options.clone()).environment().into_iter().collect();
        assert_eq!(env[CLAUDE_ARTIFACT_ENV], "1", "Claude Code withholds it from SDK sessions otherwise");
        assert!(!env.contains_key("CLAUDE_CONFIG_DIR"));
        options.config_dir = Some("/accounts/work".into());
        let env: std::collections::HashMap<_, _> = AgentLaunch::Claude(options).environment().into_iter().collect();
        assert_eq!(env[CLAUDE_ARTIFACT_ENV], "1");
        assert_eq!(env["CLAUDE_CONFIG_DIR"], "/accounts/work");
    }
}
