/**
 * A project's home: one box to start a session, the recent sessions under
 * it, and the rest (what agents read here, the workspace's details) folded
 * into rows that say what is inside before they are opened.
 */
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { PROVIDERS, PROVIDER_LABELS, parseLocalModel, type Provider, type SessionRecord, type TeamPreset } from "@thingmaker/contracts";
import { startingProvider, useStore } from "../store";
import { api } from "../ipc";
import { OutboxPanel } from "./OutboxPanel";
import { WipeDialog } from "./WipeDialog";
import { ProviderMark } from "./ProviderMark";
import { relative, RowMenu, type Row } from "./Sidebar";
import { IconArrowUp, IconBranch, IconCheck, IconChevron, IconDots } from "./icons";
import { t } from "../i18n";
import { localKindLabel } from "../localModels";

/** How many sessions the home lists before "Show all". */
const RECENT = 8;

/** The home folder as "~", so the path reads as it would in a terminal. */
export function homePath(path: string): string {
  return path.replace(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)(?=$|[\\/])/, "~");
}

function sessionLabel(record: SessionRecord): string {
  const title = record.titleOverlay || record.autoTitle;
  if (title && title !== record.agentSessionId) return title;
  return t("Session {id}", { id: record.agentSessionId.replace(/^(cx|tm)-/, "").slice(0, 8) });
}

/** A small menu that closes on Escape or a click outside it. */
function useDismiss(open: boolean, close: () => void) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) close();
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, close]);
  return root;
}

export function WorkspaceView({ workspaceId }: { workspaceId: string }) {
  const inspection = useStore((s) => s.inspections[workspaceId]);
  const inspectWorkspace = useStore((s) => s.inspectWorkspace);
  const loadRecords = useStore((s) => s.loadRecords);
  const records = useStore((s) => s.records[workspaceId]);
  const workspace = useStore((s) => s.workspaces.find((entry) => entry.id === workspaceId));
  const repo = useStore((s) => s.repoInfo[workspaceId]);
  const removeWorkspace = useStore((s) => s.removeWorkspace);
  const setError = useStore((s) => s.setError);
  const [menuOpen, setMenuOpen] = useState(false);
  const [wiping, setWiping] = useState(false);
  const closeMenu = useMemo(() => () => setMenuOpen(false), []);
  const menu = useDismiss(menuOpen, closeMenu);

  useEffect(() => {
    if (!inspection) void inspectWorkspace(workspaceId);
    else if (!records) void loadRecords(workspaceId);
  }, [inspection, workspaceId, inspectWorkspace, loadRecords, records]);

  if (!inspection) {
    return (
      <div className="panel">
        <p className="muted">{t("Inspecting workspace…")}</p>
      </div>
    );
  }

  const trusted = inspection.record.trustState === "trusted_local" && !inspection.trustStale;
  const root = inspection.identity.canonicalRoot;
  const name = (workspace?.displayPath ?? root).split(/[\\/]/).filter(Boolean).pop() ?? root;
  const kind = inspection.isGitRepository ? (inspection.isUnityProject ? t("Git · Unity") : t("Git")) : inspection.isUnityProject ? t("Unity") : t("Folder");
  const act = (run: () => void) => () => {
    run();
    setMenuOpen(false);
  };

  // Laid out like a conversation: the project and its recent sessions above,
  // the box that starts a session at the bottom.
  return (
    <div className="project-home">
      <div className="project-home-scroll">
        <div className="project-home-body">
      <header className="home-head">
        <div className="home-head-text">
          <h2>{name}</h2>
          <div className="home-meta">
            <span className="mono" title={root}>
              {homePath(workspace?.displayPath ?? root)}
            </span>
            {repo && (
              <>
                <span className="home-sep">·</span>
                <span className="home-branch mono">
                  <IconBranch size={12} />
                  {repo.head}
                </span>
              </>
            )}
            <span className="home-sep">·</span>
            <span>{kind}</span>
          </div>
        </div>
        <div className="home-menu-anchor" ref={menu}>
          <button aria-expanded={menuOpen} aria-haspopup="menu" aria-label={t("More")} className="icon-button" onClick={() => setMenuOpen(!menuOpen)} title={t("More")} type="button">
            <IconDots />
          </button>
          {menuOpen && (
            <div className="context-menu home-menu" role="menu">
              <button className="context-item" onClick={act(() => void api.revealInFinder(root).catch(setError))} role="menuitem" type="button">
                {t("Open folder")}
              </button>
              <button className="context-item" onClick={act(() => void navigator.clipboard?.writeText(root).catch(() => undefined))} role="menuitem" type="button">
                {t("Copy path")}
              </button>
              <button className="context-item" onClick={act(() => void inspectWorkspace(workspaceId))} role="menuitem" type="button">
                {t("Re-check agent setup")}
              </button>
              <button className="context-item context-item-danger" onClick={act(() => setWiping(true))} role="menuitem" type="button">
                {t("Wipe all sessions…")}
              </button>
              <button className="context-item" onClick={act(() => void removeWorkspace(workspaceId))} role="menuitem" type="button">
                {t("Remove from ThingMaker")}
              </button>
            </div>
          )}
        </div>
      </header>

      {wiping && <WipeDialog name={name} onClose={() => setWiping(false)} workspaceId={workspaceId} />}

      <OutboxPanel workspaceId={workspaceId} />

      <RecentSessions records={records ?? []} trusted={trusted} workspaceId={workspaceId} />
        </div>
      </div>

      <div className="project-home-composer">
        <StartBox name={name} trusted={trusted} workspaceId={workspaceId} />
      </div>
    </div>
  );
}

/**
 * The composer a session starts from: type and send. The provider and model,
 * or a team, are chips as in a session; what is written goes out as soon as
 * the agent is ready.
 */
function StartBox({ workspaceId, name, trusted }: { workspaceId: string; name: string; trusted: boolean }) {
  const providers = useStore((s) => s.providers);
  const presets = useStore((s) => s.teamPresets);
  const defaults = useStore((s) => s.sessionDefaults);
  const enterSends = useStore((s) => s.uiPrefs.enterSends);
  const openSession = useStore((s) => s.openSession);
  const openSessionWithPreset = useStore((s) => s.openSessionWithPreset);
  const setView = useStore((s) => s.setView);
  const [text, setText] = useState("");
  const [provider, setProvider] = useState<Provider | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [preset, setPreset] = useState<TeamPreset | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const chosen = provider ?? startingProvider(providers);
  const ready = (candidate: Provider) => !!providers?.find((entry) => entry.provider === candidate)?.resolved;

  useEffect(() => {
    input.current?.focus();
  }, [workspaceId]);

  const start = () => {
    if (!trusted) return;
    const message = text.trim() ? text : undefined;
    if (preset) void openSessionWithPreset(workspaceId, preset, message);
    else void openSession(workspaceId, { mode: "new" }, chosen, { ...(model ? { model } : {}), ...(message ? { message } : {}) });
    setText("");
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
    if (enterSends ? event.shiftKey : !event.shiftKey) return;
    event.preventDefault();
    start();
  };
  const presetReady = !preset || ready(preset.orchestrator.provider);
  const canStart = trusted && (preset ? presetReady : ready(chosen));

  return (
    <div className="home-start">
      <textarea
        aria-label={t("Prompt")}
        className="home-start-input"
        disabled={!trusted}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={onKeyDown}
        placeholder={trusted ? t("What do you want to work on in {name}?", { name }) : t("Trust this workspace to start sessions here")}
        ref={input}
        rows={2}
        value={text}
      />
      <div className="home-start-bar">
        {preset ? (
          <span className="model-chip home-chip-static" title={t("The team decides who leads")}>
            <ProviderMark provider={preset.orchestrator.provider} size={13} />
            <span className="model-chip-model">{PROVIDER_LABELS[preset.orchestrator.provider]}</span>
          </span>
        ) : (
          <ModelChoice
            model={model ?? defaults[chosen]?.model ?? (chosen === "claude" ? "opus" : null)}
            onPick={(nextProvider, nextModel) => {
              setProvider(nextProvider);
              setModel(nextModel);
            }}
            provider={chosen}
          />
        )}
        <TeamChoice onEdit={() => setView({ kind: "teams" })} onPick={setPreset} preset={preset} presets={presets} />
        <button aria-label={t("Start session")} className="send-button send-round home-start-send" disabled={!canStart} onClick={start} title={t("Start session")} type="button">
          <IconArrowUp size={14} />
        </button>
      </div>
    </div>
  );
}

function ModelChoice({ provider, model, onPick }: { provider: Provider; model: string | null; onPick: (provider: Provider, model: string | null) => void }) {
  const providers = useStore((s) => s.providers);
  const catalog = useStore((s) => s.providerModels);
  const loadProviderModels = useStore((s) => s.loadProviderModels);
  const [open, setOpen] = useState(false);
  const [browsing, setBrowsing] = useState<Provider>(provider);
  const close = useMemo(() => () => setOpen(false), []);
  const root = useDismiss(open, close);
  const available = PROVIDERS.filter((candidate) => providers?.find((entry) => entry.provider === candidate)?.resolved);
  const name = (model && catalog[provider]?.find((entry) => entry.id === model)?.name) ?? (model === "opus" ? "Opus" : model) ?? t("Default model");
  const label = (candidate: Provider, id: string | null, name: string) =>
    candidate === "local" ? `${name} · ${localKindLabel(parseLocalModel(id ?? undefined).kind)}` : name;

  useEffect(() => {
    if (open) void loadProviderModels(browsing);
  }, [open, browsing, loadProviderModels]);

  return (
    <div className="model-menu-anchor" ref={root}>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        className="model-chip"
        onClick={() => {
          setBrowsing(provider);
          setOpen(!open);
        }}
        title={t("Provider and model")}
        type="button"
      >
        <ProviderMark provider={provider} size={13} />
        <span className="model-chip-model">
          {PROVIDER_LABELS[provider]} · {label(provider, model, name)}
        </span>
        <IconChevron open size={11} />
      </button>
      {open && (
        <div className="context-menu home-choice-menu" role="menu">
          {available.map((candidate) => (
            <div key={candidate}>
              <button aria-expanded={browsing === candidate} className="context-item context-item-row" onClick={() => setBrowsing(candidate)} role="menuitem" type="button">
                <ProviderMark provider={candidate} size={13} />
                {PROVIDER_LABELS[candidate]}
              </button>
              {browsing === candidate && (
                <div className="home-choice-models">
                  {(catalog[candidate] ?? []).map((entry) => (
                    <button
                      aria-checked={candidate === provider && entry.id === model}
                      className="context-item context-item-row"
                      key={entry.id}
                      onClick={() => {
                        onPick(candidate, entry.id);
                        setOpen(false);
                      }}
                      role="menuitemradio"
                      type="button"
                    >
                      {candidate === provider && entry.id === model ? <IconCheck size={12} /> : <span className="home-choice-slot" />}
                      {label(candidate, entry.id, entry.name)}
                    </button>
                  ))}
                  {!catalog[candidate] && <span className="small muted home-choice-loading">{t("Loading models…")}</span>}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function TeamChoice({ preset, presets, onPick, onEdit }: { preset: TeamPreset | null; presets: TeamPreset[]; onPick: (preset: TeamPreset | null) => void; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  const close = useMemo(() => () => setOpen(false), []);
  const root = useDismiss(open, close);
  const pick = (next: TeamPreset | null) => () => {
    onPick(next);
    setOpen(false);
  };
  return (
    <div className="model-menu-anchor" ref={root}>
      <button aria-expanded={open} aria-haspopup="menu" className="model-chip" onClick={() => setOpen(!open)} type="button">
        <span className="model-chip-model">{preset ? preset.name : t("Set team")}</span>
        <IconChevron open size={11} />
      </button>
      {open && (
        <div className="context-menu home-choice-menu" role="menu">
          <span className="home-choice-label">{t("Start with a team")}</span>
          {preset && (
            <button className="context-item" onClick={pick(null)} role="menuitem" type="button">
              {t("No team")}
            </button>
          )}
          {presets.map((entry) => (
            <button
              aria-checked={preset?.id === entry.id}
              className="context-item context-item-row"
              key={entry.id}
              onClick={pick(entry)}
              role="menuitemradio"
              title={t("{lead} leads; {workers}", {
                lead: `${PROVIDER_LABELS[entry.orchestrator.provider]}${entry.orchestrator.model ? ` · ${entry.orchestrator.model}` : ""}`,
                workers: entry.combo.workers.map((worker) => worker.name).join(", ") || t("no workers"),
              })}
              type="button"
            >
              {preset?.id === entry.id ? <IconCheck size={12} /> : <span className="home-choice-slot" />}
              {entry.name}
            </button>
          ))}
          <button
            className="context-item muted"
            onClick={() => {
              setOpen(false);
              onEdit();
            }}
            role="menuitem"
            type="button"
          >
            {presets.length === 0 ? t("Make a team…") : t("Edit teams…")}
          </button>
        </div>
      )}
    </div>
  );
}

/** The project's sessions, newest first: the whole row opens one. */
function RecentSessions({ workspaceId, records, trusted }: { workspaceId: string; records: SessionRecord[]; trusted: boolean }) {
  const sessions = useStore((s) => s.sessions);
  const openSession = useStore((s) => s.openSession);
  const selectSession = useStore((s) => s.selectSession);
  const [all, setAll] = useState(false);
  const [menu, setMenu] = useState<{ row: Row; at: { x: number; y: number } } | null>(null);
  const closeMenu = useMemo(() => () => setMenu(null), []);
  // Workers a delegation opened belong to their orchestrator, and archived
  // sessions to the sidebar's Archived list.
  const listed = records
    .filter((record) => record.archiveState === "active" && !record.parentSessionId)
    .sort((a, b) => (b.lastSeen ?? b.createdAt) - (a.lastSeen ?? a.createdAt));
  const shown = all ? listed : listed.slice(0, RECENT);
  const liveOf = (record: SessionRecord) => Object.values(sessions).find((session) => session.snapshot.agentSessionId === record.agentSessionId && session.projection.process !== "exited");

  return (
    <section aria-labelledby="home-recent" className="home-recent">
      <div className="home-section-head">
        <h3 id="home-recent">{t("Recent")}</h3>
        {listed.length > RECENT && (
          <button className="link small" onClick={() => setAll(!all)} type="button">
            {all ? t("Show fewer") : t("Show all {count}", { count: listed.length })}
          </button>
        )}
      </div>
      {listed.length === 0 ? (
        <p className="small muted">{t("No sessions in this workspace yet.")}</p>
      ) : (
        <ul className="home-sessions">
          {shown.map((record) => {
            const live = liveOf(record);
            const running = live?.projection.foreground === "running";
            const row: Row = {
              workspaceId,
              project: "",
              id: record.agentSessionId,
              provider: record.provider,
              label: sessionLabel(record),
              liveId: live?.handle.id ?? null,
              attention: "none",
              running,
              exited: false,
              updated: undefined,
              trusted,
              pinned: record.pinned,
              archived: false,
              parent: null,
              fork: record.origin === "fork",
            };
            const openMenu = (at: { x: number; y: number }) => setMenu({ row, at });
            return (
              <li
                className="home-session-item"
                key={record.id}
                onContextMenu={(event) => {
                  event.preventDefault();
                  openMenu({ x: event.clientX, y: event.clientY });
                }}
              >
                <button
                  className="home-session"
                  disabled={!live && !trusted}
                  onClick={() => (live ? selectSession(live.handle.id) : void openSession(workspaceId, { mode: "resume", session_id: record.agentSessionId }))}
                  title={live ? undefined : trusted ? t("Resume {id}", { id: record.agentSessionId }) : t("Trust the workspace to resume")}
                  type="button"
                >
                  <ProviderMark provider={record.provider} size={15} />
                  <span className="home-session-title">{sessionLabel(record)}</span>
                  <span className="home-session-when">
                    {live && <span className={`home-live ${running ? "home-live-running" : ""}`} title={running ? t("running") : t("open")} />}
                    {running ? t("running") : relative(record.lastSeen ? new Date(record.lastSeen).toISOString() : undefined)}
                  </span>
                </button>
                <button
                  aria-label={t("Session actions")}
                  className="icon-button home-session-more"
                  onClick={(event) => {
                    const box = event.currentTarget.getBoundingClientRect();
                    openMenu({ x: box.left - 160, y: box.bottom + 4 });
                  }}
                  title={t("Session actions")}
                  type="button"
                >
                  <IconDots size={14} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {menu && <RowMenu at={menu.at} onClose={closeMenu} parentLabel={null} row={menu.row} />}
    </section>
  );
}
