import { useEffect, useState } from "react";
import { IS_REMOTE } from "./remote/mode";
import { ConnectionBanner } from "./remote/ConnectionBanner";
import { useStore } from "./store";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { WorkspaceView } from "./components/WorkspaceView";
import { SessionPanel } from "./components/SessionPanel";
import { OfficePane } from "./components/OfficePane";
import { ArtifactPanel } from "./components/ArtifactPanel";
import { UpdatePrompt } from "./components/AppUpdate";
import { StartingPanel } from "./components/StartingPanel";
import { SignInPanel } from "./components/SignInPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { AttentionBanner, CloseRequestDialog, LockedResumeDialog } from "./components/Attention";
import { useKeyboardShortcuts } from "./shortcuts";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { TeamsView } from "./components/TeamsView";
import { CommandPalette } from "./components/CommandPalette";
import { IconFolder, IconKey } from "./components/icons";
import { t } from "./i18n";
import { SetupGuide } from "./components/SetupGuide";
import { BrowserLauncher } from "./components/WebView";
import { MOD_KEY, readSetup } from "./setupProgress";


function Welcome() {
  const [finished, setFinished] = useState(() => IS_REMOTE || readSetup()?.finished === true);
  if (!finished) return <SetupGuide onFinish={() => setFinished(true)} />;
  return <WelcomeCard />;
}

function WelcomeCard() {
  const addWorkspaceByPicker = useStore((s) => s.addWorkspaceByPicker);
  const setView = useStore((s) => s.setView);
  const loadProviders = useStore((s) => s.loadProviders);
  const workspaces = useStore((s) => s.workspaces);
  const [executionBefore, executionAfter] = t(
    "Execution is {mode}: agents' tools run with your full user authority. Review a workspace's executable configuration before trusting it.",
  ).split("{mode}");
  return (
    <div className="welcome">
      <div className="welcome-card">
        <img className="welcome-brand-icon" src={`${import.meta.env.BASE_URL}thingmaker-icon.png`} alt="" draggable={false} />
        <h1>ThingMaker</h1>
        <p className="muted">{t("One desktop for Claude Code and Codex on your own subscriptions: sessions you can observe, steer, review and hand between providers.")}</p>
        <div className="welcome-actions">
          <button className="button button-primary" onClick={() => void addWorkspaceByPicker()} type="button">
            <IconFolder size={15} /> {workspaces.length === 0 ? t("Add a project folder") : t("Add another folder")}
          </button>
          <button
            className="button"
            onClick={() => {
              setView({ kind: "signin" });
              void loadProviders();
            }}
            type="button"
          >
            <IconKey size={15} /> {t("Set up providers")}
          </button>
        </div>
        {!IS_REMOTE && <BrowserLauncher />}
        <dl className="welcome-keys">
          <dt>{MOD_KEY}K</dt>
          <dd>{t("command palette")}</dd>
          <dt>{MOD_KEY}N</dt>
          <dd>{t("new session in the selected workspace")}</dd>
          <dt>{MOD_KEY}1–9</dt>
          <dd>{t("switch sessions")}</dd>
          <dt>{MOD_KEY}B</dt>
          <dd>{t("toggle sidebar")}</dd>
        </dl>
        <p className="small muted">
          {executionBefore}
          <strong>{t("Trusted local · host access")}</strong>
          {executionAfter}
        </p>
      </div>
    </div>
  );
}

/** What an error says about trying again, in words. */
function retryLabel(retry: string): string {
  switch (retry) {
    case "never":
      return t("trying again will not help");
    case "read_only":
      return t("safe to try again");
    case "after_reconcile":
      return t("try again once reconnected");
    case "user_action":
      return t("check before trying again");
    default:
      return retry;
  }
}

export function App() {
  const bootstrap = useStore((s) => s.bootstrap);
  const error = useStore((s) => s.error);
  const clearError = useStore((s) => s.clearError);
  const busy = useStore((s) => s.busy);
  const view = useStore((s) => s.view);
  const announcement = useStore((s) => s.announcement);
  const reducedMotion = useStore((s) => s.uiPrefs.reducedMotion);
  const theme = useStore((s) => s.uiPrefs.theme ?? "dark");
  const sidebarCollapsed = useStore((s) => s.sidebarCollapsed);
  const setView = useStore((s) => s.setView);
  // A quick chat's session gets the whole width: no side pane.
  const quickSession = useStore((s) => {
    if (s.view.kind !== "session") return false;
    const workspaceId = s.sessions[s.view.sessionId]?.workspaceId;
    return !!workspaceId && s.quickChats.some((chat) => chat.workspaceId === workspaceId);
  });

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  // Colour scheme lives on <html> so every CSS variable in :root switches at once.
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  const toggleSidebar = useStore((s) => s.toggleSidebar);
  useKeyboardShortcuts();

  // On the phone, Android's back closes the topmost thing — the palette,
  // a dialog, the drawer — then opens the drawer from a screen, and
  // only then leaves the app.
  useEffect(() => {
    if (!IS_REMOTE) return;
    const back = () => {
      const state = useStore.getState();
      if (state.paletteOpen) { state.setPaletteOpen(false); return true; }
      if (document.querySelector('[aria-modal="true"]')) {
        (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        return true;
      }
      if (!state.sidebarCollapsed) { state.toggleSidebar(); return true; }
      if (state.view.kind !== "welcome") { state.toggleSidebar(); return true; }
      return false;
    };
    (window as unknown as { __thingmakerBack?: () => boolean }).__thingmakerBack = back;
  }, []);

  return (
    <div className={`app ${sidebarCollapsed && !IS_REMOTE ? "sidebar-collapsed" : ""} ${IS_REMOTE ? "app-remote" : ""}`} data-reduce-motion={reducedMotion === "reduce" ? "true" : undefined}>
      <div aria-live="polite" className="sr-only" role="status">
        {announcement}
      </div>
      <CommandPalette />
      <CloseRequestDialog />
      <LockedResumeDialog />
      {!IS_REMOTE && <UpdatePrompt />}
      {IS_REMOTE && <ConnectionBanner />}
      <div className="shell">
        {!sidebarCollapsed && (
          <ErrorBoundary label={t("sidebar")}>
            <Sidebar />
          </ErrorBoundary>
        )}
        {IS_REMOTE && !sidebarCollapsed && <button aria-label={t("Close the menu")} className="mobile-sidebar-backdrop" onClick={toggleSidebar} type="button" />}
        <section className="main">
          <ErrorBoundary label={t("top bar")}>
            <TopBar />
          </ErrorBoundary>
          {error && (
            <div className="banner banner-error" role="alert">
              <strong>{error.code}</strong> {error.message}
              <span className="muted"> ({retryLabel(error.retry)})</span>{" "}
              <button className="link" onClick={clearError} type="button">
                {t("dismiss")}
              </button>
            </div>
          )}
          {busy && (
            <div className="banner banner-busy" role="status">
              {busy}…
            </div>
          )}
          <ErrorBoundary label={t("attention banner")}>
            <AttentionBanner />
          </ErrorBoundary>
          <div className="main-pane">
            <ErrorBoundary key={view.kind === "session" ? view.sessionId : view.kind} label={t("{view} view", { view: view.kind })}>
              {view.kind === "welcome" && <Welcome />}
              {view.kind === "setup" && <SetupGuide onFinish={() => setView({ kind: "welcome" })} />}
              {view.kind === "workspace" && <WorkspaceView key={view.workspaceId} workspaceId={view.workspaceId} />}
              {view.kind === "session" && (
                <div className="session-split">
                  <SessionPanel key={view.sessionId} sessionId={view.sessionId} />
                  {!IS_REMOTE && !quickSession && <OfficePane key={`office-${view.sessionId}`} sessionId={view.sessionId} />}
                </div>
              )}
              {view.kind === "starting" && <StartingPanel />}
              {view.kind === "signin" && <SignInPanel />}
              {view.kind === "settings" && <SettingsPanel />}
              {view.kind === "teams" && <TeamsView />}
            </ErrorBoundary>
          </div>
        </section>
        {!IS_REMOTE && (
          <ErrorBoundary label={t("Artifact")}>
            <ArtifactPanel />
          </ErrorBoundary>
        )}
      </div>
    </div>
  );
}
