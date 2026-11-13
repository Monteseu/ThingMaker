/**
 * Settings, in tabs that stay in view while the page scrolls. One line per
 * setting: its name on the left, its control on the right. A short hint only
 * where it prevents a mistake; longer explanations behind an ⓘ; what is
 * rarely needed folded away. Integrations is a tab here (⌘I).
 */
import { IS_REMOTE } from "../remote/mode";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  CLAUDE_ORCHESTRATOR_EFFORT,
  CLAUDE_ORCHESTRATOR_MODEL,
  PROVIDER_LABELS,
  PROVIDERS,
  type CleanupCandidate,
  type ClaudeModelChoice,
  type KeepAwakeStatus,
  type NotificationSettings,
  type Provider,
  type ProviderModel,
  type StorageReport,
} from "@thingmaker/contracts";
import { useStore, basenameOf, type SettingsTab } from "../store";
import { api } from "../ipc";
import { redactText } from "../redact";
import { chooseLanguage, languageChoice, t, type LanguageChoice } from "../i18n";
import { PhoneSection } from "./PhoneSection";
import { IntegrationsSettings } from "./IntegrationsPanel";
import { ProviderMark } from "./ProviderMark";
import { Fold, FoldItem, Segmented, Select, SettingGroup, SettingRow, Switch, type Choice } from "./settings/controls";

function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 * 1024 * 1024) return `${Math.round(n / 1024 / 1024)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

const TABS = (): { id: SettingsTab; label: string; desktopOnly?: boolean }[] => [
  { id: "general", label: t("General") },
  { id: "chat", label: t("Chat") },
  { id: "models", label: t("Default Models") },
  { id: "notifications", label: t("Notifications") },
  { id: "integrations", label: t("Integrations"), desktopOnly: true },
  { id: "phone", label: t("Phone"), desktopOnly: true },
  { id: "data", label: t("Data"), desktopOnly: true },
];

/**
 * What the Big Thing picker offers before the account's own catalog has
 * answered, and if it never does.
 */
const BUILT_IN_CLAUDE_MODELS: ClaudeModelChoice[] = [
  { value: CLAUDE_ORCHESTRATOR_MODEL, name: "Fable 5.1", needsCredits: false },
  { value: "opus", name: "Opus 5.5", needsCredits: false },
  { value: "sonnet", name: "Sonnet 5.5", needsCredits: false },
  { value: "haiku", name: "Haiku 4.5", needsCredits: false },
];

const EFFORT_LABELS = (): Record<string, string> => ({
  minimal: t("Minimal"),
  low: t("Low"),
  medium: t("Medium"),
  high: t("High"),
  xhigh: t("Extra high"),
  max: t("Max"),
  none: t("Off"),
  off: t("Off"),
});
const effortLabel = (effort: string) => EFFORT_LABELS()[effort] ?? effort;

export function SettingsPanel() {
  const settings = useStore((s) => s.settings);
  const loadSettings = useStore((s) => s.loadSettings);
  const saveSettings = useStore((s) => s.saveSettings);
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const tabs = TABS().filter((tab) => !(IS_REMOTE && tab.desktopOnly));
  const asked = view.kind === "settings" ? view.tab : undefined;
  const tab = tabs.some((entry) => entry.id === asked) ? (asked as SettingsTab) : "general";
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!settings) void loadSettings();
  }, [settings, loadSettings]);
  useEffect(() => {
    if (scroller.current) scroller.current.scrollTop = 0;
  }, [tab]);

  const update = (patch: Partial<NotificationSettings>) => {
    if (settings) void saveSettings({ ...settings, ...patch });
  };

  return (
    <div className="settings-page" ref={scroller}>
      <div className="settings-tabs-wrap">
        <nav aria-label={t("Settings")} className="settings-tabs" role="tablist">
          {tabs.map((entry) => (
            <button aria-selected={entry.id === tab} className="settings-tab" key={entry.id} onClick={() => setView({ kind: "settings", tab: entry.id })} role="tab" type="button">
              {entry.label}
            </button>
          ))}
        </nav>
      </div>
      <div className="settings-content" role="tabpanel">
        {tab === "integrations" ? (
          <IntegrationsSettings />
        ) : tab === "phone" ? (
          <PhoneSection />
        ) : tab === "data" ? (
          <DataTab />
        ) : !settings ? (
          <p className="muted small">{t("Loading settings…")}</p>
        ) : tab === "general" ? (
          <GeneralTab settings={settings} update={update} />
        ) : tab === "chat" ? (
          <ChatTab settings={settings} update={update} />
        ) : tab === "models" ? (
          <ModelsTab settings={settings} update={update} />
        ) : (
          <NotificationsTab settings={settings} update={update} />
        )}
      </div>
    </div>
  );
}

type TabProps = { settings: NotificationSettings; update: (patch: Partial<NotificationSettings>) => void };

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

function GeneralTab({ settings, update }: TabProps) {
  const uiPrefs = useStore((s) => s.uiPrefs);
  const setUiPrefs = useStore((s) => s.setUiPrefs);
  const appUpdate = useStore((s) => s.appUpdate);
  const checkForUpdate = useStore((s) => s.checkForUpdate);
  const keepAwake = useKeepAwakeStatus(settings);
  const languages: Choice<LanguageChoice>[] = [
    { value: "system", label: t("Same as the system") },
    { value: "en", label: "English" },
    { value: "es", label: "Español" },
  ];
  const closing: Choice<NotificationSettings["closeBehavior"]>[] = [
    { value: "ask", label: t("Ask each time") },
    { value: "tray", label: t("Keep running in the tray") },
    { value: "quit", label: t("Quit and stop sessions") },
  ];
  const working = keepAwake ? keepAwake.work.sessions + keepAwake.work.goals + keepAwake.work.jobs : 0;
  return (
    <>
      <SettingGroup>
        <SettingRow name={t("Language")} hint={t("Changing it reloads the window; sessions carry on")}>
          <Select choices={languages} label={t("Language")} onChange={chooseLanguage} value={languageChoice()} />
        </SettingRow>
        {!IS_REMOTE && (
          <>
            <SettingRow
              hint={
                appUpdate.error
                  ? appUpdate.error
                  : appUpdate.available
                    ? t("Version {version} is available", { version: appUpdate.available.version })
                    : appUpdate.current
                      ? t("Up to date")
                      : t("Not checked yet")
              }
              info={t("Updates come from ThingMaker's GitHub releases and are checked against its signing key before they install.")}
              name={appUpdate.current ? `ThingMaker ${appUpdate.current}` : "ThingMaker"}
            >
              <button className="set-button" disabled={appUpdate.checking} onClick={() => void checkForUpdate("manual")} type="button">
                {appUpdate.checking ? t("Checking…") : t("Check now")}
              </button>
            </SettingRow>
            <SettingRow name={t("Offer new versions when it starts")}>
              <Switch checked={uiPrefs.updateReminder !== false} label={t("Offer new versions when it starts")} onChange={(on) => void setUiPrefs({ updateReminder: on })} />
            </SettingRow>
          </>
        )}
      </SettingGroup>

      <SettingGroup title={t("While agents work")}>
        <SettingRow
          hint={keepAwake ? (working > 0 ? t("Awake for {n} working now", { n: working }) : t("Nothing is working")) : undefined}
          info={t(
            "Only idle sleep is held off: the display still sleeps, and closing the lid or choosing Sleep still works. A Big Thing waiting for a limit to reset counts as working, so it is awake to carry on.",
          )}
          name={t("Keep this computer awake")}
        >
          <Switch checked={settings.keepAwake} label={t("Keep this computer awake")} onChange={(on) => update({ keepAwake: on })} />
        </SettingRow>
        {settings.keepAwake && (
          <SettingRow hint={keepAwake?.onBattery ? t("On battery now") : undefined} name={t("On battery too")} sub>
            <Switch checked={settings.keepAwakeOnBattery} label={t("On battery too")} onChange={(on) => update({ keepAwakeOnBattery: on })} />
          </SettingRow>
        )}
        <SettingRow name={t("Closing the window")}>
          <Select choices={closing} label={t("Closing the window")} onChange={(closeBehavior) => update({ closeBehavior })} value={settings.closeBehavior} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("Accessibility")}>
        <SettingRow
          hint={t("Your system setting is always honoured")}
          info={t("Streamed text is not announced; screen readers hear finished turns, questions and helpers stopping. Every action can be reached from the keyboard and the command palette (⌘K).")}
          name={t("Reduce motion")}
        >
          <Switch checked={uiPrefs.reducedMotion === "reduce"} label={t("Reduce motion")} onChange={(on) => void setUiPrefs({ reducedMotion: on ? "reduce" : "system" })} />
        </SettingRow>
      </SettingGroup>
    </>
  );
}

/** The host's own last keep-awake decision, read every few seconds while shown. */
function useKeepAwakeStatus(settings: NotificationSettings): KeepAwakeStatus | null {
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  useEffect(() => {
    if (IS_REMOTE) return;
    let live = true;
    const read = () =>
      api
        .keepAwakeRefresh()
        .then((next) => live && setStatus(next))
        .catch(() => undefined);
    void read();
    const timer = setInterval(read, 5000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [settings.keepAwake, settings.keepAwakeOnBattery]);
  return status;
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

/** Providers that can run here: installed, and not signed out. */
function useReadyProviders(): Provider[] {
  const providers = useStore((s) => s.providers);
  return useMemo(
    () => PROVIDERS.filter((provider) => {
      const info = providers?.find((entry) => entry.provider === provider);
      return !!info?.resolved && info.auth?.loggedIn !== false;
    }),
    [providers],
  );
}

/** Each ready provider's model catalog, loaded once and shared through the store. */
function useCatalogs(providers: Provider[]): Partial<Record<Provider, ProviderModel[]>> {
  const catalogs = useStore((s) => s.providerModels);
  const loadProviderModels = useStore((s) => s.loadProviderModels);
  useEffect(() => {
    for (const provider of providers) if (!catalogs[provider]) void loadProviderModels(provider).catch(() => undefined);
  }, [providers.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps
  return catalogs;
}

const SHORTCUTS = (): [string, string][] => [
  [t("Command palette"), "⌘K"],
  [t("New session"), "⌘N"],
  [t("Add a project"), "⌘O"],
  [t("Switch session"), "⌘1–9"],
  [t("Focus the prompt"), "⌘L"],
  [t("Search the transcript"), "⌘F"],
  [t("Back / forward"), "⌘[ ⌘]"],
  [t("Sidebar"), "⌘B"],
  [t("Integrations"), "⌘I"],
  [t("Sign-in"), "⌘,"],
  [t("Settings"), "⌘;"],
];

function ChatTab({ settings, update }: TabProps) {
  const uiPrefs = useStore((s) => s.uiPrefs);
  const setUiPrefs = useStore((s) => s.setUiPrefs);
  const ready = useReadyProviders();
  const catalogs = useCatalogs(ready);

  // Reply suggestions: one menu of every ready provider's models, or off.
  const suggestion = settings.replySuggestionModel ? `${settings.replySuggestionProvider ?? "claude"}::${settings.replySuggestionModel}` : "";
  const suggestionChoices: Choice<string>[] = [{ value: "", label: t("Off") }];
  for (const provider of ready) {
    for (const model of catalogs[provider] ?? []) {
      if (provider === "local" && model.id.startsWith("@agent/")) continue;
      suggestionChoices.push({ value: `${provider}::${model.id}`, label: `${PROVIDER_LABELS[provider]} · ${model.name}`, ...(model.needsCredits ? { hint: t("uses credits") } : {}) });
    }
  }
  if (suggestion && !suggestionChoices.some((choice) => choice.value === suggestion)) {
    suggestionChoices.push({ value: suggestion, label: `${PROVIDER_LABELS[settings.replySuggestionProvider ?? "claude"]} · ${settings.replySuggestionModel}` });
  }

  const pageSizes = [25, 50, 100, 200, uiPrefs.transcriptPage].filter((n, at, all) => all.indexOf(n) === at).sort((a, b) => a - b);
  return (
    <>
      <SettingGroup>
        <SettingRow hint={uiPrefs.enterSends ? t("Shift+Enter adds a new line") : t("⌘+Enter sends; Enter adds a new line")} name={t("Enter sends")}>
          <Switch checked={uiPrefs.enterSends} label={t("Enter sends")} onChange={(on) => void setUiPrefs({ enterSends: on })} />
        </SettingRow>
        <SettingRow
          info={t(
            "⌘ with the send key does the other one. Steering needs an agent that supports it (Claude Code and Codex do); otherwise the message is queued. A queue sends by itself after a turn that finished, and waits after one that was cancelled or failed.",
          )}
          name={t("Sending while the agent works")}
        >
          <Segmented
            choices={[
              { value: "queue", label: t("Queue"), hint: t("Held until the turn is over") },
              { value: "steer", label: t("Steer"), hint: t("Sent into the running turn") },
            ]}
            label={t("Sending while the agent works")}
            onChange={(followUp) => void setUiPrefs({ followUp })}
            value={uiPrefs.followUp ?? "queue"}
          />
        </SettingRow>
        <SettingRow
          hint={t("Tab accepts, Esc dismisses")}
          info={t("A suggestion reads only the last reply, your last message and the reply before it.")}
          name={t("Reply suggestions")}
        >
          <Select
            choices={suggestionChoices}
            label={t("Reply suggestions")}
            onChange={(value) => {
              if (!value) return update({ replySuggestionModel: "" });
              const [provider, ...model] = value.split("::");
              update({ replySuggestionProvider: provider as Provider, replySuggestionModel: model.join("::") });
            }}
            value={suggestion}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup>
        <Fold name={t("Keyboard shortcuts")}>
          <div className="set-shortcuts">
            {SHORTCUTS().map(([name, keys]) => (
              <span key={name}>
                {name} <kbd>{keys}</kbd>
              </span>
            ))}
          </div>
        </Fold>
        <Fold name={t("Advanced")}>
          <FoldItem
            actions={
              <Select
                choices={pageSizes.map((n) => ({ value: String(n), label: String(n) }))}
                label={t("Messages loaded at a time")}
                onChange={(value) => void setUiPrefs({ transcriptPage: Number(value) })}
                value={String(uiPrefs.transcriptPage)}
              />
            }
          >
            <span className="set-item-text">{t("Messages loaded at a time")}</span>
            <span className="set-item-note">{t("Earlier ones load as you scroll up")}</span>
          </FoldItem>
        </Fold>
      </SettingGroup>
    </>
  );
}

// ---------------------------------------------------------------------------
// Default Models
// ---------------------------------------------------------------------------

function ModelsTab({ settings, update }: TabProps) {
  const sessionDefaults = useStore((s) => s.sessionDefaults);
  const setSessionDefault = useStore((s) => s.setSessionDefault);
  const resetSessionDefaults = useStore((s) => s.resetSessionDefaults);
  const saveSettings = useStore((s) => s.saveSettings);
  const ready = useReadyProviders();
  const catalogs = useCatalogs(ready);
  const anyChosen = Object.values(sessionDefaults).some((choice) => choice?.model || choice?.reasoningEffort);

  // Big Thing's model: the account's catalog when it answers, the built-in
  // list otherwise.
  const claudeCatalog = catalogs.claude;
  const claudeModels: ClaudeModelChoice[] = claudeCatalog?.length
    ? claudeCatalog.map((model) => ({ value: model.id, name: model.name, needsCredits: model.needsCredits }))
    : BUILT_IN_CLAUDE_MODELS;
  const goalModel = settings.claudeOrchestratorModel ?? CLAUDE_ORCHESTRATOR_MODEL;
  const goalModels: Choice<string>[] = claudeModels.map((choice) => ({ value: choice.value, label: choice.name, ...(choice.needsCredits ? { hint: t("uses credits") } : {}) }));
  if (!goalModels.some((choice) => choice.value === goalModel)) goalModels.push({ value: goalModel, label: goalModel });

  const budget = settings.odysseyTokenBudget;
  const budgets = [1_000_000, 2_000_000, 5_000_000, 10_000_000, 20_000_000, 50_000_000];
  const budgetChoices: Choice<string>[] = [
    { value: "", label: t("No limit") },
    ...[...new Set([...budgets, ...(budget ? [budget] : [])])].sort((a, b) => a - b).map((n) => ({ value: String(n), label: t("{n}M tokens", { n: Math.round((n / 1_000_000) * 10) / 10 }) })),
  ];

  return (
    <>
      <SettingGroup
        action={
          anyChosen ? (
            <button className="set-link" onClick={() => void resetSessionDefaults()} type="button">
              {t("Reset to the account's defaults")}
            </button>
          ) : undefined
        }
        note={t("Changing the model or effort in a session also makes it the default here.")}
        title={t("New sessions")}
      >
        {ready.length === 0 && <SettingRow hint={t("Install and sign in to a provider first")} name={t("No providers ready")} />}
        {ready.map((provider) => (
          <ProviderDefaultRow
            catalog={catalogs[provider]}
            choice={sessionDefaults[provider] ?? {}}
            key={provider}
            onChange={(patch) => void setSessionDefault(provider, patch)}
            provider={provider}
          />
        ))}
      </SettingGroup>

      <SettingGroup title={t("Big Thing goals")}>
        <SettingRow
          hint={t("When Claude runs the goal")}
          info={t("A goal keeps the settings it started with, so changing these never loosens a goal that is already running.")}
          name={t("Model and effort")}
        >
          <Select choices={goalModels} label={t("Goal model")} onChange={(claudeOrchestratorModel) => update({ claudeOrchestratorModel })} value={goalModel} />
          <Select
            choices={["low", "medium", "high", "xhigh", "max"].map((level) => ({ value: level, label: effortLabel(level) }))}
            label={t("Goal effort")}
            onChange={(claudeOrchestratorEffort) => update({ claudeOrchestratorEffort })}
            value={settings.claudeOrchestratorEffort ?? CLAUDE_ORCHESTRATOR_EFFORT}
          />
        </SettingRow>
        <SettingRow info={t("Big Thing stops after this many continuations, whatever state the goal is in. Keep it low until you have watched a full run.")} name={t("Stop after")}>
          <input
            aria-label={t("Continuation limit")}
            className="set-number"
            inputMode="numeric"
            min={1}
            onChange={(event) => {
              const value = Number.parseInt(event.target.value, 10);
              if (Number.isFinite(value) && value > 0) update({ odysseyMaxContinuations: value });
            }}
            type="number"
            value={settings.odysseyMaxContinuations ?? 10}
          />
          <span className="set-unit">{t("continuations")}</span>
        </SettingRow>
        <SettingRow info={t("Paid input and output, counted from the agent's own transcript. With no limit the spend is still recorded.")} name={t("Token budget")}>
          <Select
            choices={budgetChoices}
            label={t("Token budget")}
            onChange={(value) => {
              if (value) return update({ odysseyTokenBudget: Number(value) });
              // No limit is the absent field, not a zero.
              const { odysseyTokenBudget: _cleared, ...rest } = settings;
              void saveSettings(rest);
            }}
            value={budget ? String(budget) : ""}
          />
        </SettingRow>
      </SettingGroup>
    </>
  );
}

function ProviderDefaultRow({
  provider,
  catalog,
  choice,
  onChange,
}: {
  provider: Provider;
  catalog: ProviderModel[] | undefined;
  choice: { model?: string; reasoningEffort?: string };
  onChange: (patch: { model?: string; reasoningEffort?: string }) => void;
}) {
  const models = (catalog ?? []).filter((model) => !(provider === "local" && model.id.startsWith("@agent/")));
  // Claude with nothing chosen starts on Opus (store.ts launchDefaults).
  const value = choice.model ?? (provider === "claude" ? "opus" : "");
  const modelChoices: Choice<string>[] = [
    ...(provider === "claude" ? [] : [{ value: "", label: t("Account default") }]),
    ...models.map((model) => ({ value: model.id, label: model.name, ...(model.needsCredits ? { hint: t("uses credits") } : {}) })),
  ];
  if (value && !modelChoices.some((entry) => entry.value === value)) modelChoices.push({ value, label: value === "opus" ? "Opus" : value });
  const model = models.find((entry) => entry.id === value) ?? models.find((entry) => entry.isDefault);
  const efforts = model?.efforts ?? [];
  const effort = choice.reasoningEffort ?? "";
  const effortChoices: Choice<string>[] = [
    { value: "", label: model?.defaultEffort ? t("Default ({effort})", { effort: effortLabel(model.defaultEffort) }) : t("Default") },
    ...efforts.map((level) => ({ value: level, label: effortLabel(level) })),
  ];
  if (effort && !effortChoices.some((entry) => entry.value === effort)) effortChoices.push({ value: effort, label: effortLabel(effort) });
  return (
    <SettingRow
      name={
        <>
          <ProviderMark provider={provider} size={16} />
          {PROVIDER_LABELS[provider]}
        </>
      }
    >
      <Select choices={modelChoices} label={t("{provider} model", { provider: PROVIDER_LABELS[provider] })} onChange={(next) => onChange({ model: next })} placeholder={t("Loading…")} value={value} />
      {(efforts.length > 0 || effort) && (
        <Select choices={effortChoices} label={t("{provider} effort", { provider: PROVIDER_LABELS[provider] })} onChange={(next) => onChange({ reasoningEffort: next })} value={effort} />
      )}
    </SettingRow>
  );
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

const QUIET_START = "22:00";
const QUIET_END = "08:00";

function NotificationsTab({ settings, update }: TabProps) {
  const workspaces = useStore((s) => s.workspaces);
  const testNotification = useStore((s) => s.testNotification);
  const quiet = settings.quietHoursStart !== settings.quietHoursEnd;
  const muted = settings.mutedWorkspaceIds.filter((id) => workspaces.some((workspace) => workspace.id === id)).length;
  const toggleMuted = (id: string) =>
    update({
      mutedWorkspaceIds: settings.mutedWorkspaceIds.includes(id) ? settings.mutedWorkspaceIds.filter((w) => w !== id) : [...settings.mutedWorkspaceIds, id],
    });
  return (
    <>
      <SettingGroup>
        <SettingRow
          hint={t("They never include your prompts or code")}
          info={t("macOS asks for permission the first time one is actually shown.")}
          name={t("Notifications")}
        >
          {settings.enabled && (
            <button className="set-link" onClick={() => void testNotification()} type="button">
              {t("Send a test")}
            </button>
          )}
          <Switch checked={settings.enabled} label={t("Notifications")} onChange={(enabled) => update({ enabled })} />
        </SettingRow>
        {settings.enabled && (
          <>
            <SettingRow name={t("A turn finished")} sub>
              <Switch checked={settings.notifyCompleted} label={t("A turn finished")} onChange={(notifyCompleted) => update({ notifyCompleted })} />
            </SettingRow>
            <SettingRow name={t("A turn failed or a helper stopped")} sub>
              <Switch checked={settings.notifyFailed} label={t("A turn failed or a helper stopped")} onChange={(notifyFailed) => update({ notifyFailed })} />
            </SettingRow>
            <SettingRow name={t("An agent needs your answer")} sub>
              <Switch checked={settings.notifyNeedsInput} label={t("An agent needs your answer")} onChange={(notifyNeedsInput) => update({ notifyNeedsInput })} />
            </SettingRow>
          </>
        )}
      </SettingGroup>

      {settings.enabled && (
        <SettingGroup>
          <SettingRow name={t("Quiet hours")}>
            <Switch
              checked={quiet}
              label={t("Quiet hours")}
              onChange={(on) => update(on ? { quietHoursStart: QUIET_START, quietHoursEnd: QUIET_END } : { quietHoursStart: "00:00", quietHoursEnd: "00:00" })}
            />
          </SettingRow>
          {quiet && (
            <SettingRow name={t("From")} sub>
              <input aria-label={t("Quiet hours start")} className="set-time" onChange={(event) => event.target.value && update({ quietHoursStart: event.target.value })} type="time" value={settings.quietHoursStart} />
              <span className="set-unit">{t("to")}</span>
              <input aria-label={t("Quiet hours end")} className="set-time" onChange={(event) => event.target.value && update({ quietHoursEnd: event.target.value })} type="time" value={settings.quietHoursEnd} />
            </SettingRow>
          )}
          <Fold name={t("Muted projects")} summary={muted === 0 ? t("None") : t("{n} muted", { n: muted })}>
            {workspaces.length === 0 && <p className="set-item-note">{t("No projects yet.")}</p>}
            {workspaces.map((workspace) => {
              const name = basenameOf(workspace.displayPath);
              return (
                <FoldItem actions={<Switch checked={settings.mutedWorkspaceIds.includes(workspace.id)} label={t("Mute {name}", { name })} onChange={() => toggleMuted(workspace.id)} />} key={workspace.id}>
                  <span className="set-item-text" title={workspace.displayPath}>
                    {name}
                  </span>
                </FoldItem>
              );
            })}
          </Fold>
        </SettingGroup>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

const SEGMENT_COLORS = ["var(--info)", "#b49cf2", "var(--ok)", "var(--warn)", "var(--muted)"];

function DataTab() {
  const setError = useStore((s) => s.setError);
  const [report, setReport] = useState<StorageReport | null>(null);
  const [candidates, setCandidates] = useState<CleanupCandidate[] | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const load = () => api.storageInspect().then(setReport).catch(setError);
  useEffect(() => {
    void load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const preview = async () => {
    try {
      const list = await api.cleanupPreview();
      setCandidates(list);
      setChecked(new Set(list.map((c) => c.path)));
    } catch (error) {
      setError(error);
    }
  };
  const run = async () => {
    try {
      await api.cleanupRun([...checked]);
      setCandidates(null);
      await load();
    } catch (error) {
      setError(error);
    }
  };

  if (!report) return <p className="muted small">{t("Measuring…")}</p>;
  const entries = [...report.entries].sort((a, b) => b.bytes - a.bytes);
  const total = entries.reduce((sum, entry) => sum + entry.bytes, 0);
  const top = entries.slice(0, 4).filter((entry) => entry.bytes > 0);
  const rest = total - top.reduce((sum, entry) => sum + entry.bytes, 0);
  const segments = [...top.map((entry) => ({ name: entry.name, bytes: entry.bytes })), ...(rest > 0 ? [{ name: t("Other"), bytes: rest }] : [])];

  return (
    <>
      <section className="set-group">
        <div className="set-rows set-storage">
          <div className="set-storage-head">
            <strong>{bytes(total)}</strong>
            <span className="set-hint">{t("ThingMaker's own data on this computer")}</span>
          </div>
          <div aria-label={t("Storage by area")} className="set-bar" role="img">
            {segments.map((segment, index) => (
              <span key={segment.name} style={{ width: `${total ? (segment.bytes / total) * 100 : 0}%`, background: SEGMENT_COLORS[index] }} title={`${segment.name} · ${bytes(segment.bytes)}`} />
            ))}
          </div>
          <div className="set-legend">
            {segments.map((segment, index) => (
              <span key={segment.name}>
                <i style={{ background: SEGMENT_COLORS[index] }} />
                {segment.name}
                <b>{bytes(segment.bytes)}</b>
              </span>
            ))}
          </div>
          <div className="set-actions">
            <button className="set-button" onClick={() => void preview()} type="button">
              {t("Clean up…")}
            </button>
            <button className="set-button" onClick={() => void api.revealInFinder(report.dataDir).catch(setError)} type="button">
              {t("Show in Finder")}
            </button>
            <button className="set-link" onClick={() => void load()} type="button">
              {t("Measure again")}
            </button>
          </div>
          {candidates && (
            <div className="set-cleanup">
              {candidates.length === 0 ? (
                <p className="set-item-note">{t("Nothing is old enough to clean up.")}</p>
              ) : (
                <>
                  {candidates.map((c) => (
                    <label className="set-cleanup-item" key={c.path} title={c.path}>
                      <input
                        checked={checked.has(c.path)}
                        onChange={() => {
                          const next = new Set(checked);
                          if (next.has(c.path)) next.delete(c.path);
                          else next.add(c.path);
                          setChecked(next);
                        }}
                        type="checkbox"
                      />
                      <span className="set-item-text">{basenameOf(c.path)}</span>
                      <span className="set-item-note">
                        {bytes(c.bytes)} · {c.reason}
                      </span>
                    </label>
                  ))}
                  <div className="set-actions">
                    <button className="set-button set-button-warn" disabled={checked.size === 0} onClick={() => void run()} type="button">
                      {t("Delete {count} selected", { count: checked.size })}
                    </button>
                    <button className="set-link" onClick={() => setCandidates(null)} type="button">
                      {t("Cancel")}
                    </button>
                  </div>
                </>
              )}
            </div>
          )}
        </div>
      </section>

      <SettingGroup>
        <Fold name={t("What's kept and for how long")}>
          <div className="set-table-wrap">
            <table className="set-table">
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.name}>
                    <td title={entry.path}>{entry.name}</td>
                    <td>{bytes(entry.bytes)}</td>
                    <td>{entry.policy}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {report.notes.map((note) => (
            <p className="set-item-note" key={note}>
              {note}
            </p>
          ))}
          <p className="set-item-note mono">{report.dataDir}</p>
        </Fold>
      </SettingGroup>

      <SupportBundle />
    </>
  );
}

function SupportBundle() {
  const setError = useStore((s) => s.setError);
  const [preview, setPreview] = useState<{ text: string; redactions: number } | null>(null);
  const generate = async () => {
    try {
      const bundle = await api.supportBundlePreview();
      const { text, redactions } = redactText(JSON.stringify(bundle, null, 2));
      setPreview({ text, redactions: redactions.reduce((n, r) => n + r.count, 0) });
    } catch (error) {
      setError(error);
    }
  };
  const save = async () => {
    if (!preview) return;
    try {
      await api.saveTextFile(`thingmaker-support-${new Date().toISOString().slice(0, 10)}.json`, preview.text);
    } catch (error) {
      setError(error);
    }
  };
  return (
    <SettingGroup>
      <SettingRow
        hint={preview ? (preview.redactions === 1 ? t("1 secret-looking value hidden") : t("{count} secret-looking values hidden", { count: preview.redactions })) : undefined}
        info={t(
          "Versions, the state of live sessions and recent warnings, with your home folder hidden. Never prompts, transcripts, environment variables or tokens. Nothing is uploaded: you read it, then save it yourself.",
        )}
        name={t("Support bundle")}
      >
        {preview ? (
          <>
            <button className="set-link" onClick={() => setPreview(null)} type="button">
              {t("Close")}
            </button>
            <button className="set-button" onClick={() => void save()} type="button">
              {t("Save…")}
            </button>
          </>
        ) : (
          <button className="set-button" onClick={() => void generate()} type="button">
            {t("Create…")}
          </button>
        )}
      </SettingRow>
      {preview && <textarea aria-label={t("Support bundle")} className="set-bundle" readOnly value={preview.text} />}
    </SettingGroup>
  );
}
