/**
 * The first-run guide: connect a provider, add a project folder, optionally
 * load the default teams, then start the first chat. Each step ticks itself
 * off from the app's real state (a signed-in provider, a trusted folder);
 * where someone is between restarts is kept by setupProgress.
 *
 * Install runs npm for the providers that install that way (Claude's
 * adapter and Codex); Gemini and Ollama are downloads, so those rows link to
 * them. Sign-in is the provider's own, as on the Providers page.
 */
import { useEffect, useMemo, useState } from "react";
import { PROVIDER_LABELS, type Provider, type ProviderInfo, type TeamPreset, type WorkspaceInspection } from "@thingmaker/contracts";
import { api } from "../ipc";
import { basenameOf, useStore } from "../store";
import { t } from "../i18n";
import { DEFAULT_TEAMS, missingDefaultTeams } from "../defaultTeams";
import { MOD_KEY, readSetup, writeSetup, type SetupProgress } from "../setupProgress";
import { ProviderMark } from "./ProviderMark";
import { StartBox } from "./WorkspaceView";
import { IconCheck, IconFolder } from "./icons";

type RowState = "checking" | "missing" | "signedout" | "signing" | "ready";

/** The npm package each installable provider comes from; mirrors the supervisor's install_package. */
const NPM_PACKAGE: Partial<Record<Provider, string>> = { claude: "@agentclientprotocol/claude-agent-acp", codex: "@openai/codex" };
const IS_MAC = MOD_KEY === "⌘";

const USES = (): Record<Provider, string> => ({
  claude: t("Uses your Claude Pro, Max, Team or Enterprise plan"),
  codex: t("Uses your ChatGPT plan"),
  gemini: t("Signs in through the Antigravity app"),
  copilot: "GitHub Copilot CLI (copilot login)",
  local: t("Runs on this computer through Ollama. Free and private, no account needed."),
});

/** Where to get the providers ThingMaker cannot install itself. */
const DOWNLOAD: Partial<Record<Provider, { url: string; label: () => string }>> = {
  gemini: { url: "https://antigravity.google", label: () => t("Get Antigravity") },
  local: { url: "https://ollama.com/download", label: () => t("Get Ollama") },
};

function rowState(info: ProviderInfo | undefined, signingIn: boolean): RowState {
  if (!info) return "checking";
  if (!info.resolved) return "missing";
  if (signingIn) return "signing";
  if (info.auth?.loggedIn === true) return "ready";
  if (info.auth?.loggedIn === false) return "signedout";
  return "checking";
}

export function isReady(providers: readonly ProviderInfo[] | null, provider: Provider): boolean {
  return providers?.find((entry) => entry.provider === provider)?.auth?.loggedIn === true;
}

/** Ollama is on this computer but not answering: it can be started from here. */
export function ollamaStopped(providers: readonly ProviderInfo[] | null): boolean {
  const info = providers?.find((entry) => entry.provider === "local");
  return Boolean(info?.resolved) && info?.auth?.loggedIn !== true;
}

/** The same start the Providers page has: Ollama's app on a Mac, `ollama serve` elsewhere. */
function StartOllama({ small }: { small?: boolean }) {
  const loadProviders = useStore((s) => s.loadProviders);
  const [starting, setStarting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const start = async () => {
    setStarting(true);
    setProblem(null);
    try {
      await api.localStart();
      await loadProviders();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String((error as { message?: unknown })?.message ?? error));
    } finally {
      setStarting(false);
    }
  };
  return (
    <button className={`button ${small ? "button-small" : "button-primary button-small"}`} disabled={starting} onClick={() => void start()} title={problem ?? undefined} type="button">
      {starting ? t("Starting…") : problem ? t("Try again") : t("Start Ollama")}
    </button>
  );
}

function installCommand(provider: Provider): string | null {
  const name = NPM_PACKAGE[provider];
  if (name) return `npm install -g ${name}`;
  if (provider === "gemini" && IS_MAC) return "curl -fsSL https://antigravity.google/cli/install.sh | bash";
  return null;
}

function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="setup-cmd">
      <code>{command}</code>
      <button
        className="button button-small"
        onClick={() => {
          void navigator.clipboard?.writeText(command).then(
            () => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            },
            () => undefined,
          );
        }}
        type="button"
      >
        {copied ? t("Copied") : t("Copy")}
      </button>
    </div>
  );
}

function ProviderLine({ provider }: { provider: Provider }) {
  const providers = useStore((s) => s.providers);
  const signIn = useStore((s) => s.signIn);
  const startSignIn = useStore((s) => s.startSignIn);
  const cancelSignIn = useStore((s) => s.cancelSignIn);
  const loadProviders = useStore((s) => s.loadProviders);
  const openUrl = useStore((s) => s.openUrl);
  const [installing, setInstalling] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [showCommand, setShowCommand] = useState(false);
  const info = providers?.find((entry) => entry.provider === provider);
  const run = signIn?.provider === provider && !signIn.account ? signIn : null;
  const signingIn = run?.state === "starting" || run?.state === "running";
  const state = installing ? "missing" : rowState(info, signingIn);
  const command = installCommand(provider);
  const download = DOWNLOAD[provider];

  const install = async () => {
    setInstalling(true);
    setProblem(null);
    try {
      await api.providerInstall(provider);
      await loadProviders();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : typeof error === "object" && error && "message" in error ? String((error as { message: unknown }).message) : String(error));
    } finally {
      setInstalling(false);
    }
  };

  const pill = installing ? (
    <span className="setup-pill busy">{t("Installing…")}</span>
  ) : provider === "copilot" && info?.resolved ? (
    <span className="setup-pill">{t("Sign-in managed by CLI")}</span>
  ) : state === "ready" ? (
    <span className="setup-pill ok">{info?.auth?.plan ? t("Ready · {plan}", { plan: info.auth.plan }) : t("Ready")}</span>
  ) : state === "missing" ? (
    <span className="setup-pill warn">{t("Not on this computer")}</span>
  ) : state === "signedout" ? (
    <span className="setup-pill warn">{provider === "local" ? t("Ollama is stopped") : t("Not signed in")}</span>
  ) : state === "signing" ? (
    <span className="setup-pill busy">{t("Finish in your browser")}</span>
  ) : (
    <span className="setup-pill">{t("Checking…")}</span>
  );

  const hint = installing
    ? t("Running the official installer. This can take a minute.")
    : provider === "copilot" && info?.resolved
      ? t("Run copilot in your terminal, then use /login. Sign-in is managed by the CLI.")
    : state === "missing" && NPM_PACKAGE[provider]
      ? t("ThingMaker drives the official {name} program, which isn't installed yet.", { name: PROVIDER_LABELS[provider] })
      : state === "signing"
        ? t("A browser tab opens for the sign-in. Finish there and come back.")
        : state === "signedout" && provider === "gemini"
          ? t("Sign in inside the Antigravity app, then check again.")
          : state === "signedout" && provider === "local"
            ? t("Ollama is installed but not running. Start it to use local models.")
            : USES()[provider];

  let action = null;
  if (installing) action = null;
  else if (state === "missing" && NPM_PACKAGE[provider])
    action = (
      <button className="button button-primary button-small" onClick={() => void install()} type="button">
        {t("Install")}
      </button>
    );
  else if (state === "missing" && download)
    action = (
      <button className="button button-small" onClick={() => void openUrl(download.url)} type="button">
        {download.label()}
      </button>
    );
  else if (state === "signedout" && (provider === "claude" || provider === "codex"))
    action = (
      <button className="button button-primary button-small" disabled={signIn?.state === "running"} onClick={() => void startSignIn(provider)} type="button">
        {t("Sign in")}
      </button>
    );
  else if (state === "signedout" && provider === "local") action = <StartOllama />;
  else if (state === "signedout")
    action = (
      <button className="button button-small" onClick={() => void loadProviders()} type="button">
        {t("Check again")}
      </button>
    );
  else if (state === "signing")
    action = (
      <>
        {run?.urls.map((url) => (
          <button className="button button-primary button-small" key={url} onClick={() => void openUrl(url)} title={url} type="button">
            {t("Open the sign-in page")}
          </button>
        ))}
        <button className="button button-small" onClick={() => void cancelSignIn()} type="button">
          {t("Cancel")}
        </button>
      </>
    );

  return (
    <div className="setup-provider">
      <div className="setup-provider-main">
        <ProviderMark provider={provider} size={28} />
        <div className="setup-provider-label">
          <span className="setup-provider-name">
            <b>{PROVIDER_LABELS[provider]}</b>
            {pill}
          </span>
          <span className="setup-hint">{hint}</span>
        </div>
        <div className="setup-provider-actions">
          {state === "missing" && !installing && command && (
            <button className="link small" onClick={() => setShowCommand(!showCommand)} type="button">
              {showCommand ? t("Hide command") : t("Command")}
            </button>
          )}
          {action}
        </div>
      </div>
      {(problem || run?.state === "failed" || run?.state === "timed_out" || (showCommand && command && state === "missing")) && (
        <div className="setup-provider-drawer">
          {problem && (
            <p className="setup-problem" role="alert">
              {problem}
              {problem.includes("nodejs.org") && (
                <>
                  {" "}
                  <button className="link" onClick={() => void openUrl("https://nodejs.org")} type="button">
                    {t("Get Node.js")}
                  </button>
                </>
              )}
            </p>
          )}
          {(run?.state === "failed" || run?.state === "timed_out") && <p className="setup-problem">{run.state === "failed" ? t("The sign-in did not finish.") : t("The sign-in timed out.")}</p>}
          {showCommand && command && state === "missing" && (
            <>
              <p className="setup-hint">{t("Prefer to run it yourself? Paste this in a terminal, then come back. ThingMaker checks again on its own.")}</p>
              <CopyCommand command={command} />
            </>
          )}
        </div>
      )}
    </div>
  );
}

function Step({ id, title, sub, state, optional, onChange, children }: { id: string; title: string; sub: string; state: "done" | "current" | "later"; optional?: boolean; onChange?: () => void; children?: React.ReactNode }) {
  return (
    <section aria-current={state === "current" ? "step" : undefined} className={`setup-step ${state}`}>
      <div className="setup-step-head">
        <span aria-hidden="true" className="setup-step-num">
          {state === "done" ? <IconCheck size={13} /> : id}
        </span>
        <span className="setup-step-title">
          <b>
            {title}
            {optional && <span className="setup-optional">{t("Optional")}</span>}
          </b>
          <span>{sub}</span>
        </span>
        {state === "done" && onChange && (
          <button className="link small" onClick={onChange} type="button">
            {t("Change")}
          </button>
        )}
      </div>
      {state === "current" && children && <div className="setup-step-body">{children}</div>}
    </section>
  );
}

/** What a folder's inspection found, as short chips. */
function foundChips(inspection: WorkspaceInspection): string[] {
  const chips: string[] = [];
  if (inspection.isGitRepository) chips.push(t("Git repository"));
  if (inspection.isUnityProject) chips.push(t("Unity project"));
  const kinds = new Set(inspection.sources.filter((source) => source.present).map((source) => source.kind));
  if (kinds.has("mcp")) chips.push(t("MCP servers"));
  if (kinds.has("instructions")) chips.push(t("Agent instructions"));
  if (kinds.has("claude-settings")) chips.push(t("Claude settings"));
  if (kinds.has("codex-config")) chips.push(t("Codex settings"));
  return chips;
}

function FolderStep({ workspaceId, onPick }: { workspaceId: string | null; onPick: (id: string | null) => void }) {
  const addWorkspaceByPicker = useStore((s) => s.addWorkspaceByPicker);
  const inspection = useStore((s) => (workspaceId ? s.inspections[workspaceId] : undefined));
  const trust = useStore((s) => s.trust);
  const setView = useStore((s) => s.setView);
  const busy = useStore((s) => s.busy);

  if (!workspaceId) {
    return (
      <>
        <div>
          <button className="button button-primary" onClick={() => void addWorkspaceByPicker({ stay: true }).then((id) => id && onPick(id))} type="button">
            <IconFolder size={15} /> {t("Choose a folder…")}
          </button>
        </div>
        <p className="setup-hint">{t("Any folder with code or documents works. Before an agent runs there, ThingMaker looks for anything in it that would run on its own and shows you, so you decide whether to trust it.")}</p>
      </>
    );
  }
  if (!inspection) return <div className="setup-card">{busy ?? t("Looking through the folder…")}</div>;
  const name = basenameOf(inspection.identity.displayPath);
  const runs = inspection.sources.filter((source) => source.present && source.executable).length;
  return (
    <div className="setup-card">
      <div className="setup-card-head">
        <IconFolder size={15} />
        <b>{name}</b>
        {runs === 0 ? <span className="setup-pill ok">{t("Nothing runs on its own")}</span> : <span className="setup-pill warn">{runs === 1 ? t("1 item can run commands") : t("{count} items can run commands", { count: runs })}</span>}
      </div>
      {foundChips(inspection).length > 0 && (
        <div className="setup-chips">
          {foundChips(inspection).map((chip) => (
            <span className="chip" key={chip}>
              {chip}
            </span>
          ))}
        </div>
      )}
      <p className="setup-hint">{t("Agents here run with your permissions: they can read and change files and run commands, the same as you could in a terminal.")}</p>
      <div className="setup-actions">
        <button className="button button-primary button-small" onClick={() => void trust(workspaceId, "trusted_local")} type="button">
          {t("Trust and continue")}
        </button>
        <button className="link small" onClick={() => setView({ kind: "workspace", workspaceId })} type="button">
          {t("See details")}
        </button>
        <button className="link small" onClick={() => onPick(null)} type="button">
          {t("Pick another folder")}
        </button>
      </div>
    </div>
  );
}

function TeamStep({ onLoaded, onSkip }: { onLoaded: () => void; onSkip: () => void }) {
  const providers = useStore((s) => s.providers);
  const addTeamPresets = useStore((s) => s.addTeamPresets);
  const setView = useStore((s) => s.setView);
  const needs = (entry: (typeof DEFAULT_TEAMS)[number]) => {
    const used = new Set<Provider>([entry.orchestrator.provider, ...entry.combo.workers.map((worker) => worker.provider)]);
    return [...used].filter((provider) => !isReady(providers, provider));
  };
  const load = async () => {
    const added = missingDefaultTeams(useStore.getState().teamPresets);
    if (added.length > 0 && !(await addTeamPresets(added))) return;
    onLoaded();
  };
  return (
    <>
      <p className="setup-hint">{t("A team is one lead agent that hands parts of the work to helpers on other models.")}</p>
      <p className="setup-hint">
        <b>{t("Teams aren't required.")}</b> {t("You can use a single model at a time. It can still start subagents the way it does in its official app, just not across providers.")}
      </p>
      <div className="setup-list">
        {DEFAULT_TEAMS.map((entry) => {
          const missing = needs(entry);
          const onlyStoppedOllama = missing.length === 1 && missing[0] === "local" && ollamaStopped(providers);
          return (
            <div className="setup-team" key={entry.name}>
              <span className="setup-faces">
                {[entry.orchestrator.provider, ...entry.combo.workers.map((worker) => worker.provider)].map((provider, index) => (
                  <ProviderMark key={index} provider={provider} size={18} />
                ))}
              </span>
              <span className="setup-team-label">
                <b>{entry.name}</b>
                <span>{entry.combo.workers.length === 1 ? t("1 helper") : t("{count} helpers", { count: entry.combo.workers.length })}</span>
              </span>
              {missing.length === 0 ? (
                <span className="setup-pill ok">{t("Ready to use")}</span>
              ) : onlyStoppedOllama ? (
                <StartOllama small />
              ) : (
                <span className="setup-pill warn">{t("Needs {providers}", { providers: missing.map((provider) => (provider === "local" ? "Ollama" : PROVIDER_LABELS[provider])).join(", ") })}</span>
              )}
            </div>
          );
        })}
      </div>
      <div className="setup-actions">
        <button className="button button-primary button-small" onClick={() => void load()} type="button">
          {t("Load the {count} default teams", { count: DEFAULT_TEAMS.length })}
        </button>
        <button className="button button-small" onClick={() => setView({ kind: "teams" })} type="button">
          {t("Build my own")}
        </button>
        <button className="link small" onClick={onSkip} type="button">
          {t("Skip")}
        </button>
      </div>
    </>
  );
}

export function SetupGuide({ onFinish }: { onFinish: () => void }) {
  const providers = useStore((s) => s.providers);
  const workspaces = useStore((s) => s.workspaces);
  const presets = useStore((s) => s.teamPresets);
  const loadProviders = useStore((s) => s.loadProviders);
  const selectWorkspace = useStore((s) => s.selectWorkspace);
  const [progress, setProgress] = useState<SetupProgress>(() => readSetup() ?? { finished: false });
  const [workspaceId, setWorkspaceId] = useState<string | null>(() => workspaces[0]?.id ?? null);
  const [moreAsked, setMore] = useState(false);
  // A ready Gemini or Ollama counts as connected, so it is never hidden.
  const more = moreAsked || isReady(providers, "gemini") || isReady(providers, "local") || Boolean(providers?.find((entry) => entry.provider === "copilot")?.resolved);
  const save = (patch: Partial<SetupProgress>) => setProgress(writeSetup(patch));
  const finish = () => {
    writeSetup({ finished: true });
    onFinish();
  };

  const ready = (["claude", "codex", "gemini", "local"] as Provider[]).filter((provider) => isReady(providers, provider));
  const copilotInstalled = Boolean(providers?.find((entry) => entry.provider === "copilot")?.resolved);
  const providerDone = (ready.length > 0 || copilotInstalled) && progress.continued === true;
  const workspace = workspaces.find((entry) => entry.id === workspaceId) ?? null;
  const folderDone = providerDone && workspace?.trustState === "trusted_local";
  const teamDone = folderDone && progress.team !== undefined;
  const done = [providerDone, folderDone].filter(Boolean).length;

  // Ask again while step 1 is open: an install run in a terminal, or a
  // sign-in finished in another app, shows up on its own.
  useEffect(() => {
    void loadProviders();
    if (providerDone) return;
    const timer = setInterval(() => void loadProviders(), 10_000);
    const onFocus = () => void loadProviders();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [providerDone, loadProviders]);

  // The team the first chat starts with: the first default one whose
  // providers are all signed in, when the defaults were loaded.
  const startingTeam = useMemo<TeamPreset | null>(() => {
    if (progress.team !== "loaded") return null;
    for (const entry of DEFAULT_TEAMS) {
      const preset = presets.find((candidate) => candidate.name === entry.name);
      const used = [preset?.orchestrator.provider, ...(preset?.combo.workers.map((worker) => worker.provider) ?? [])];
      if (preset && used.every((provider) => provider && isReady(providers, provider))) return preset;
    }
    return null;
  }, [progress.team, presets, providers]);

  const readyNames = [...ready.map((provider) => PROVIDER_LABELS[provider]), ...(copilotInstalled ? [PROVIDER_LABELS.copilot] : [])].join(", ");

  return (
    <div className="setup">
      <div className="setup-inner">
        <header className="setup-hello">
          <img alt="" className="setup-logo" draggable={false} src={`${import.meta.env.BASE_URL}thingmaker-icon.png`} />
          <h1>{t("Welcome to ThingMaker")}</h1>
          <p>{t("Claude Code, Codex and more in one window, on the subscriptions you already pay for. A few steps and you're chatting.")}</p>
        </header>

        <div className="setup-progress">
          <span>{t("{done} of 3 done", { done })}</span>
          <span className="setup-track">
            <span className="setup-fill" style={{ width: `${(done / 3) * 100}%` }} />
          </span>
        </div>

        <div className="setup-steps">
          <Step
            id="1"
            onChange={() => save({ continued: false })}
            state={providerDone ? "done" : "current"}
            sub={providerDone ? readyNames : ready.length > 0 ? t("{names} ready. Add more, or continue.", { names: ready.map((provider) => PROVIDER_LABELS[provider]).join(", ") }) : t("One is enough. You can add more later.")}
            title={t("Connect a provider")}
          >
            <div className="setup-list">
              <ProviderLine provider="claude" />
              <ProviderLine provider="codex" />
              {more ? (
                <>
                  <ProviderLine provider="gemini" />
                  <ProviderLine provider="copilot" />
                  <ProviderLine provider="local" />
                </>
              ) : (
                <button className="setup-more" onClick={() => setMore(true)} type="button">
                  {t("More: Gemini, GitHub Copilot, local models")}
                </button>
              )}
            </div>
            {(ready.length > 0 || copilotInstalled) && (
              <div className="setup-actions">
                <button className="button button-primary" onClick={() => save({ continued: true })} type="button">
                  {t("Continue")}
                </button>
                <span className="setup-hint">{t("Add another now, or later from Providers.")}</span>
              </div>
            )}
          </Step>

          <Step
            id="2"
            onChange={() => setWorkspaceId(null)}
            state={folderDone ? "done" : providerDone ? "current" : "later"}
            sub={folderDone && workspace ? t("{name} · trusted", { name: basenameOf(workspace.displayPath) }) : t("The folder your agents will work in.")}
            title={t("Add a project folder")}
          >
            <FolderStep onPick={setWorkspaceId} workspaceId={workspaceId} />
          </Step>

          <Step
            id="3A"
            onChange={() => save({ team: undefined })}
            optional
            state={teamDone ? "done" : folderDone ? "current" : "later"}
            sub={progress.team === "loaded" ? t("Default teams added") : progress.team === "skipped" ? t("Skipped. Teams are always a click away in the sidebar.") : t("Pick a premade team, or skip and chat with one agent.")}
            title={t("Create your first team")}
          >
            <TeamStep onLoaded={() => save({ team: "loaded" })} onSkip={() => save({ team: "skipped" })} />
          </Step>

          <Step id="3B" state={teamDone ? "current" : "later"} sub={t("Ask for anything, in plain words.")} title={t("Start your first chat")}>
            {workspace && (
              <>
                <StartBox initialPreset={startingTeam} name={basenameOf(workspace.displayPath)} onStarted={finish} trusted workspaceId={workspace.id} />
                <div className="setup-actions">
                  <button
                    className="button button-primary"
                    onClick={() => {
                      finish();
                      void selectWorkspace(workspace.id);
                    }}
                    type="button"
                  >
                    {t("I'll chat later!")}
                  </button>
                  <span className="setup-hint">{t("Finishes the setup. Your project is in the sidebar whenever you're ready.")}</span>
                </div>
              </>
            )}
          </Step>
        </div>

        <button className="link small setup-skip" onClick={finish} type="button">
          {t("Skip for now")}
        </button>

        <div className="setup-after">
          <h3>{t("What else it can do")}</h3>
          <div className="setup-tour">
            <div>
              <b>{t("Teams")}</b>
              <span>{t("Several agents on one task, each with a role.")}</span>
            </div>
            <div>
              <b>{t("Big Thing")}</b>
              <span>{t("A long goal, worked through step by step.")}</span>
            </div>
            <div>
              <b>{t("Ultra-Review")}</b>
              <span>{t("Two providers review a change and debate it.")}</span>
            </div>
            <div>
              <b>{t("Phone")}</b>
              <span>{t("Follow and steer sessions from your Android.")}</span>
            </div>
          </div>
          <div className="setup-keys">
            <span>
              <kbd>{MOD_KEY}K</kbd> {t("commands")}
            </span>
            <span>
              <kbd>{MOD_KEY}N</kbd> {t("new session")}
            </span>
            <span>
              <kbd>{MOD_KEY}1–9</kbd> {t("switch sessions")}
            </span>
            <span>
              <kbd>{MOD_KEY}B</kbd> {t("sidebar")}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
