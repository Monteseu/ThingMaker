# ThingMaker architecture

The technical companion to the [README](../README.md): how the app is built,
what each provider runs, and where things live.

Tauri 2 host, React/TypeScript interface, and a Rust supervisor that drives
each provider's **own official program** out of process:

| Provider | Program | Protocol | Signs in with |
| --- | --- | --- | --- |
| Claude Code | `claude-agent-acp` (Anthropic's Agent SDK over ACP) | ACP v1 | Claude Pro, Max, Team or Enterprise |
| Codex | `codex app-server` | Codex app-server JSON-RPC, bridged to ACP inside the supervisor | ChatGPT plan |
| Gemini (worker or plain session) | Antigravity CLI `agy` (`stream-json` mode) | NDJSON events, bridged to ACP inside the supervisor | Google account (Antigravity quota), shared with the Antigravity app |

Subscriptions only, never API keys. The desktop never reads, stores or passes
on a provider token: each program signs in through its provider's own flow and
keeps its credentials in its own store. Decisions are in
[`docs/adr`](adr/README.md); the providers' terms behind them are summarised in
[ADR-009](adr/ADR-009-providers-through-official-programs.md).


## Status by provider and feature

Kit is removed; Claude Code and Codex run end to end and either can orchestrate the other; Gemini runs as a worker or a plain session.

- **Supervisor** (`crates/thingmaker-supervisor`): stdio JSON-RPC transport; an
  ACP v1 client (`acp`) with capability negotiation and `session/update`
  decoding; providers (`agents`) with their launch contracts, sign-in, model
  catalogs, subagent and quota translation into provider-neutral events, and
  transcript usage readers; session actors, SQLite storage, review, git,
  worktrees, terminal, attachments, artifacts, skills and instruction files.
- **Claude Code**: sessions, resume (`session/load`), steering
  (`_session/steering`), quota (`_claude/rateLimit` → `Quota` events, sent
  only when the limit state changes), subagents from `Agent`/`Task` calls,
  transcript token usage, streamed subscription sign-in and the account's model
  catalog.
- **Codex**: `codex app-server` bridged to ACP inside the supervisor
  (`agents::codex::bridge`). Threads are sessions, with resume, steering
  (`turn/steer`) and interrupt. Approvals are answered from workspace trust.
  Rate limits arrive as `Quota` events on every turn, and a spent account fails
  the turn as `rate_limit`. Also: collab subagents, rollout token usage,
  `codex login` sign-in, `model/list` and `account/rateLimits/read`.
- **Gemini** (`agents::gemini`): Antigravity's official `agy` in `stream-json`
  mode, bridged to ACP.
  - One process per conversation, resumed with `--conversation`. A model,
    effort or mode change relaunches it between turns, and so does a cancel,
    because SIGINT ends the process.
  - A trusted workspace runs `--mode accept-edits
    --dangerously-skip-permissions` (headless `agy` otherwise refuses every
    shell command); an inspected one runs `--mode plan`.
  - Models come from `agy models`.
  - No steering and no quota signal: a spent account shows as a refused turn.
  - **A worker or a plain session, never an orchestrator.** `agy` reads MCP
    servers only from its global configuration, so it cannot take a session's
    own `team` server.
- **Accounts**: the Providers panel's **Switch account** signs a provider out
  through its own CLI, then starts a new sign-in. The launch options also accept
  a separate `CODEX_HOME` for each Codex account; the UI does not offer that yet.
- **Big Thing** (`bigthing`, ADR-010) is an engine in the supervisor that
  runs every long-horizon goal, whether or not a window is open.
  - It decides, briefs, continues, reads each settled turn, checkpoints, runs
    checks, parks on a spent window, resumes after it, and moves a run to
    another account. Every decision goes into the goal's record before it
    acts, and a running goal's session is reopened after a restart.
  - The protocol is `team` tools (`bigthing_report`, `bigthing_task`,
    `bigthing_ask`, `bigthing_amend`, `bigthing_propose_plan`), checked
    against the record when they are called; the `BIGTHING-*` lines are the
    fallback. `bigthing_load_document` hands a document the session wrote
    to its Big Thing tab, for the user to set a goal up from.
  - Shared project memory (`memory_read`, `memory_write`) and the run's
    `board`, for the orchestrator and, on a reduced server, its workers.
  - Optional: a run's own branch and worktree (checkpoints commit, rollback,
    merge back); runner dispatch of ready tasks to workers by capability, with
    a review on another provider; a spend forecast from measured turn costs,
    and a spread policy across accounts.
  - A live test runs a goal end to end on Claude and on Codex:
    `THINGMAKER_REAL_BIGTHING=1`.
- **Teams and delegation** (`delegation`): every session is an orchestrator.
  - Its agent gets a `team` MCP server with `list_workers`, `delegate`,
    `await_jobs`, `job_status` and `cancel_job`.
  - Workers are sessions on any provider and model. The **team** button beside
    the model picker opens the session's team panel: pick a preset or edit the
    orchestrator and workers directly. The team can change mid-session.
  - **Teams** (the icon beside Settings) keeps presets: an orchestrator
    (provider, model, effort) and its workers. One preset can be the default
    for new sessions, and any preset can start a session from the workspace
    view.
  - Routing goes by worker name, then capability, then quota headroom. A
    worker refused for its limit before doing any work hands its job on.
  - **Shared member resources (local pilot):** the member inspector
    offers inherited configuration or an explicit selection of skills and
    MCP servers, grouped by plugin, user and project. References are saved
    with teams and helper presets; the actual account and job workspace
    resolve them at launch. Missing/disabled references prevent launch.
    The host passes `skills`, `strictMcpConfig` and resolved `mcpServers`
    through ACP SDK options, preserving ThingMaker's internal team server.
    Skills in user/project `.agents/skills`, `.codex/skills` and user
    `.codex/skills/.system` are included alongside native `.claude` skills
    and installed Claude plugin skills. Each entry shows its source. Claude
    imports only selected external skill directories through temporary,
    skill-only local plugins with unique namespaces. Symlinks preserve
    supporting files; the actor owns the temporary directories until exit.
    Native subagents are disabled on customized workers because the SDK's
    skill allowlist scopes the main agent. Manual-only/forked skills are
    unavailable in this pilot. Codex workers use the same selection and a
    separate app-server process per worker: extra discovery roots and
    per-thread `skills.config` overrides enable selected canonical paths.
    A frozen selected catalog replaces automatic skill instructions so
    installing another skill cannot expand an existing member's listing.
    The per-thread `mcp_servers` root override contains only selected
    servers and ThingMaker's internal team server. Apps and inherited
    plugins are disabled in that worker. No skills/config/write or global
    config edit is used. Empty MCP confirmation forms on selected servers
    use the workspace's existing approval gate; data-entry forms are refused.
    Gemini and Local Models retain inherited resources.
    Account, policy, selected skill text and resolved MCP configuration
    changes prevent reuse of an old worker conversation; stale follow-ups
    ask for a new job. Running jobs keep their starting configuration.
  - Selecting a skill reveals its MCP dependencies in the same inspector.
    The detector reads `dependencies.tools` (`type: mcp`) from `SKILL.json`,
    `agents/openai.yaml`/`.yml` and SKILL.md frontmatter, plus the explicit
    `mcp-servers: [server-name]` frontmatter convention. These declarations
    are required unless the tool entry sets `optional: true`. Concrete
    `mcp__server__tool` references, MCP-named prose, resource URI schemes
    and known server function calls in SKILL.md produce suggestions only.
    Names and declared endpoints match existing definitions; endpoint and
    credential values stay private in Rust. Unavailable candidates remain
    visible with their reason, including Codex's unsupported SSE transport.
    No server is automatically enabled: the person activates a candidate
    per member or batches only unambiguous, nonconflicting definitions.
    Choosing a different same-named origin replaces its conflicting binding;
    removing a skill keeps connections already selected. Missing or ambiguous
    declared requirements prevent launch until a matching MCP is selected.
    Metadata changes invalidate the worker's resource revision. Parse/read
    problems are shown as detection notes; the detector does not execute
    scripts, install servers, infer undeclared workflows, or scan linked docs.
  - The pilot requires Claude ACP 0.106.0+ and Agent SDK 0.3.322+, the local
    combination verified with the real adapter. Run
    `python3 scripts/verify-claude-resource-filter.py --adapter /path/to/claude-agent-acp/dist/index.js`
    for an opt-in scratch-workspace probe using a few subscription tokens.
    It verifies local/plugin skills, rejected excluded skills, selected and
    internal MCP calls, excluded project/plugin servers never starting,
    and a second independent session with no optional resources.
    `python3 scripts/verify-codex-resource-filter.py` verifies the real
    Codex transcript's skill instructions and selected/excluded MCPs.
    `THINGMAKER_REAL_MEMBER_RESOURCES=1 cargo test -p thingmaker-supervisor --test real_member_resources`
    exercises the production resolver, bridge and actor against real Codex,
    reading a shared skill's supporting file and invoking its selected MCP.
    Codex was verified on 0.167.0-alpha.4.1; missing discovery/config APIs
    fail startup instead of falling back to inherited resources. The shared
    catalog includes native Claude and Codex MCP definitions, with server
    secrets resolved only in Rust. Same-named MCP definitions require one
    chosen origin. OAuth sign-ins and available tools remain provider-specific.
    Legacy commands, enterprise/synced skills, Codex plugin-only directories
    and cloud connectors are not enumerated. Skill bodies are not translated
    between harnesses: provider-specific instructions or tool dependencies
    may still need adaptation.
    The selection is a model-context/tool filter, not a filesystem sandbox:
    Read/Bash, slash dispatch, plugin hooks and managed policy are separate.
    Teams exports containing resource policies use format version 2 so older
    importers cannot silently discard restrictions; other exports retain v1.
  - Jobs show in the Agents view, and worker sessions sit under their
    orchestrator in the sidebar.
- **Forks** (`session/fork`; `storage::forks`, `commands::forks`): a copy of
  a session, nested under it (`origin = 'fork'`, `parent_session_id`).
  - Claude Code copies up to any of its messages (claude-agent-acp's fork
    point); Codex copies whole threads; otherwise a new session is briefed.
    A fork into a worktree brings Claude's transcript beside that folder's.
  - At the end of each turn a fork's final answer is held for its parent and
    rides in front of the parent's next message as context; the
    `report_to_parent` tool sends one on purpose. Nothing is steered into a
    turn or starts one.
  - A side chat is a fork linked to its parent's Big Thing goal.
  - Switching the orchestrator to the other provider is a handoff: a new
    session with the same team and a brief in its composer.

Next steps:

- Separate accounts per provider (`CODEX_HOME` per account) in the UI.
- Charging worker turns to a goal's token budget.

## Layout

```text
apps/desktop/                 Tauri 2 app: src/ (React), src-tauri/ (thin command layer)
crates/thingmaker-supervisor/  privileged engine: transport, acp, agents, supervisor, storage, review, workspace, security
packages/contracts/           desktop API TypeScript types (mirror the supervisor's serde output)
packages/test-fixtures/       mock ACP peers for the providers, captured from the real programs
runtime/skills, runtime/agents  the Big Thing skill and the Claude Code delegate definition
docs/                         architecture, decisions (adr/), release notes
```

## Prerequisites (development)

- Rust stable (see `rust-toolchain.toml`); Xcode command line tools on macOS.
- Node 22.12+ (Node 24 LTS via nvm is used; `.nvmrc`) and pnpm 10. jsdom rejects Node 25.
- Python 3 for the mock ACP peers used by tests.
- **Claude Code:** `npm install -g @agentclientprotocol/claude-agent-acp`
  (found under nvm, Homebrew or /usr/local, or set `THINGMAKER_CLAUDE_ACP` to
  its `dist/index.js`, or choose it in Providers).
- **Codex:** the `codex` CLI on PATH, or the ChatGPT app (its bundled
  `codex-cli` is found automatically), or `THINGMAKER_CODEX`.

## Commands

```bash
cargo test --workspace --exclude thingmaker-desktop
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
pnpm install && pnpm typecheck && pnpm test
```

```bash
pnpm dev
```

## Look and feel

The window uses an overlay title bar (macOS traffic lights sit over the
sidebar header, which is a drag region), a collapsible sidebar (⌘B) with
workspace groups, session rows tagged with the provider they spend, hover
actions (pin, archive, rename), an "Archived (N)" disclosure, and a footer
with each provider's quota and quick access to Providers and Settings. Every
session is durable in its agent's own transcript; "archive" hides a session
from the list and is reversible, never a deletion.

## Renderer hardening notes

The WebView runs under a strict CSP (`script-src 'self'`, workers from our own
origin only, IPC as the sole connect target). Tauri's `freezePrototype` is
deliberately off: freezing `Object.prototype` makes the common
`namespace.toString = fn` pattern throw in strict mode (xterm.js does this at
module load, blanking the window), and the CSP already prevents foreign script
from reaching the page.

## Message send latency

The composer clears immediately and shows a local pending message. This is
presentation state, never a fabricated history event. The supervisor's message
or queue entry replaces it by request ID. A failed or uncertain send restores
the original text, attachments and mentions alongside any newer draft; it is
never retried automatically.

Before dispatch, review still captures a baseline. Sessions sharing a root
share a content cache, updated only when a prompt needs a baseline. There is
no background warm-up or periodic rescan: idle sessions do no snapshot work,
and intermediate edits between prompts are not stored by the cache. Sending
rechecks the tree and ignore rules and rereads changed files. Reuse requires
matching size, mode, mtime, device, inode and nanosecond ctime, so restoring
mtime or replacing a file does not hide an edit. Platforms without that change
stamp fall back to full reads. The first send starts with a cold cache; large
trees still incur a metadata walk per send. Provider response time is separate.
Existing blobs are retained; this change does not introduce garbage collection.

Native logs include `submission prepared`, `prompt review baseline ready`,
`dispatching session prompt` and `submission completed` timings. The renderer
logs the Send-to-acceptance duration without message contents. Snapshot debug
logs also show how many files were reused. To compare full and cached capture:

```bash
SNAPSHOT_TIMING_ROOT=/path/to/workspace cargo test -p thingmaker-supervisor review::baseline::tests::snapshot_timing --lib -- --ignored --nocapture
```

## Non-goals

No API-key billing, no reading or proxying provider tokens, no rotation across
several accounts of one provider to get past its limits, no TUI scraping, no
hidden model calls behind deterministic buttons, no claims of sandboxing.
