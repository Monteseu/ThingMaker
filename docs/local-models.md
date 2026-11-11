# Local Models

Models installed in [Ollama](https://ollama.com) on this computer, as a
fourth provider beside Claude Code, Codex and Gemini. **Free:** nothing is
billed and no account is used. **Private:** what a local session reads,
writes, runs or says stays on this computer.

## Two ways to run a model

| | Chat | Agent |
|---|---|---|
| What runs | ThingMaker's own small ACP agent (`--thingmaker-local-chat`), which passes messages to Ollama's `/api/chat`. | Claude Code's adapter with Ollama as its model (`ANTHROPIC_BASE_URL=http://127.0.0.1:11434`). |
| Tools | None. | Files, commands, the task list, a local team. |
| Speed (M4, 16 GB) | Seconds per reply. | Minutes per task: every step re-reads about 4,000 tokens of agent instructions. |
| Which models | Any installed model. | Models whose Ollama capabilities include `tools`. |
| Model ids | `@chat/<name>` | `@agent/<name>` |

A chat can be continued as an agent on the same model. ThingMaker asks first,
then carries the conversation over. For models that think (`qwen3`, …), a
chat's effort control is **Think** (off by default). Agents are always asked
not to think (`/no_think`), because thinking takes most of a turn on a laptop.

## How privacy is kept

- **Chats** only connect to Ollama on `127.0.0.1`.
- **Agents** run in a sandbox that allows connections to this computer and
  nothing else: no telemetry, web, `git push` or package installs. Everything
  they start is held to it too.
  - On macOS this is `sandbox-exec`.
  - On Windows it is an AppContainer with no network capabilities. Reaching
    Ollama needs a one-time loopback exemption, approved by an administrator
    from the Local Models card.
  - With no sandbox available, agents do not start; there is no fallback.
- **Claude Code's background traffic is off for agents** (telemetry, error
  reports, feature flags, update checks). They use their own settings folder
  in ThingMaker's data folder, never `~/.claude`, so the person's Claude login,
  MCP servers and plugins are not used. Web tools are not offered.
- **A local lead hires only local helpers**, and Big Thing never moves a local
  goal to another provider.
- **A local helper under a cloud lead**, or a local seat in a mixed
  Ultra-Review panel, is marked **free, not private**: its tasks and results
  pass through the cloud.
- **The phone** shows local sessions like any other, through the person's
  relay or tunnel.

## Setup

The Local Models card in Providers walks through it: get Ollama, start it, and
download a model. The recommended list is part of ThingMaker, filtered by this
computer's memory, and never fetched. Ollama's `:cloud` models are hidden.
Each model has a context window, chosen on the card: 16k, 32k and 64k below
the model's maximum, then the maximum itself, each rated by whether it fits
this computer's memory (the model plus its KV cache for that window). The
default is the largest that fits. Chats send it with each request; agents use
a copy of the model carrying it (`<model>-thingmaker-32k`), which ThingMaker
makes and hides, because Ollama loads models with 4,096 tokens by default.

Local models share one GPU, so one local job runs at a time; the rest show
"Waiting for the local model".

## Tests

- `tests/local_chat.rs`: a chat through the session actor against a stand-in
  Ollama, resumed with its history.
- `tests/real_local.rs`: with `THINGMAKER_REAL_LOCAL=<model>`, a real agent
  writes a file from inside the sandbox.
- `agents/local/sandbox.rs`: the macOS sandbox reaches this computer and
  nothing else.
- `thingmaker-appcontainer`: quoting and parsing; the Windows code is
  type-checked with `cargo check --target x86_64-pc-windows-msvc` and has not
  yet run on Windows.
