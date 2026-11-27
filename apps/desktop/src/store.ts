/**
 * Transient UI state (Zustand). Not an authority for agent history: the
 * native supervisor owns session state and this store only holds renderer
 * projections, drafts, view selection and attention markers.
 *
 * Multiple sessions stay live at once (F01): switching the view never stops a
 * session actor. Attention is raised for sessions the user is not looking at
 * when a turn settles, an agent asks for input, or its process exits.
 */
import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AmendmentRecord,
  ContinuityArchive,
  DiffScope,
  NewAmendment,
  AttachmentSnapshot,
  ContextBundle,
  Mention,
  ProviderChoice,
  SessionDefaults,
  UiPrefs,
  DesktopError,
  EventEnvelope,
  NotificationSettings,
  OutboxEntry,
  ProfileInfo,
  SessionHandle,
  SessionRecord,
  Snapshot,
  TrustState,
  WorkspaceInspection,
  WorkspaceRecord,
  QuickChat,
  UsageSnapshot,
  GoalEdit,
  MilestoneEdit,
  NewOdyssey,
  OdysseyView,
  PlanChangeRecord,
  QuestionRecord,
  ApprovalDecision,
  AvailableUpdate,
  StepEdit,
  RepositoryInfo,
  StepState,
  WorkspaceNotes,
  LoginEvent,
  Provider,
  ProviderInfo,
  QuotaSnapshot,
  QueueOp,
  ProviderVersion,
  TurnPhase,
} from "@thingmaker/contracts";
import { EMPTY_COMBO, JOB_EVENT, TEAM_CHAT_EVENT, type TeamChatView, ULTRA_REVIEW_EVENT, type UltraReview, type UrDecision, type UrStartRequest, SESSION_TITLE_EVENT, type SessionTitleChange, ODYSSEY_STATE_NOTE, PROVIDERS, PROVIDER_LABELS, BIGTHING_EVENT, asConfigOptions, ACCOUNT_PROVIDERS, isEffortOption, isRetiredModel, localAgentHarness, parseLocalModel, type LocalSessionSetup, type AccountView, type Combo, type JobView, type ProviderModel, type BigThingEvent, type BigThingRuntime, type NestedReport, type TeamPreset, type RenameReport, type WipeReport, type WorkerSlot } from "@thingmaker/contracts";
import { isLocalChat } from "./localModels";
import { HANDOFF_SPLIT, handoffBrief, hasConversation, helperPresetFrom, readHelperPresets, readPresets, upsertHelperPreset, upsertJob, upsertPreset, withoutHandoff, type HelperPreset } from "./team";
import { copiesItself, forkBranch, forkTitle, type ForkPoint } from "./fork";
import { sessionUsesActiveAccount } from "./quotaRecovery";
import { continuityBrief, continuityMarkdown } from "./continuity";

/** Preference keys (scope `ui`) for saved teams. */
const TEAM_PRESETS_KEY = "teamPresets";
const HELPER_PRESETS_KEY = "helperPresets";
const DEFAULT_PRESET_KEY = "defaultTeamPreset";
const PROJECT_TEAM_PRESETS_KEY = "projectTeamPresets";
let projectTeamWrite = Promise.resolve();
import { t } from "./i18n";
import { api } from "./ipc";
import { notify, shouldNotify } from "./notifications";
import { CLAUDE_RETRY_MS, claudeUsageFrom } from "./odysseyClaudeQuota";
import { applyEvent, emptyProjection, EMPTY_QUEUE, type Card, type Projection } from "./projection";
import { readSetup, writeSetup } from "./setupProgress";
import { IS_REMOTE } from "./remote/mode";
import { idleSweep, loadTouches, saveTouches, touch } from "./idleSessions";
import { navEntry, navTarget, recordVisit, type NavEntry } from "./navHistory";
import { rememberedPlace, rememberPlace, type Place } from "./remote/lastPlace";

export const CLOSE_REQUESTED_EVENT = "thingmaker://close-requested";

export type AttentionKind = "none" | "completed" | "failed" | "needs_input";

/** Change of one rate-limit window between two samples. `delta` is null when the window reset in between. */
export type WindowDelta = { before: number; after: number; delta: number | null; reset: boolean };
/**
 * Account-level OpenAI usage sampled around this session's turns. The
 * account counters are shared by everything using the subscription, so the
 * deltas are "usage during this session's turns", not a precise cost.
 */
export type SessionUsage = {
  turnStart: UsageSnapshot | null;
  latest: UsageSnapshot | null;
  lastTurn: { primary: WindowDelta | null; secondary: WindowDelta | null; settledAt: number } | null;
  totalPrimary: number;
  totalSecondary: number;
  turns: number;
  resets: number;
  sampling: boolean;
  error: string | null;
};

export const EMPTY_SESSION_USAGE: SessionUsage = { turnStart: null, latest: null, lastTurn: null, totalPrimary: 0, totalSecondary: 0, turns: 0, resets: 0, sampling: false, error: null };

/** ThingMaker's own updates (REL-03): what the last check said, and an install under way. */
export type AppUpdateState = {
  /** The version running now, once a check has said. */
  current: string | null;
  available: AvailableUpdate | null;
  checking: boolean;
  checkedAt: number | null;
  /** Why the last check or install failed. */
  error: string | null;
  installing: { downloaded: number; total: number | null } | null;
  /** The startup popup is waiting: an update was found at startup and the reminder is on. */
  prompt: boolean;
};

const UPDATE_CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

export type LiveSession = {
  localSetup?: LocalSessionSetup;
  handle: SessionHandle;
  workspaceId: string;
  snapshot: Snapshot;
  projection: Projection;
  inFlightRequestId: string | null;
  /** Local feedback only; the supervisor's echo/queue replaces it. */
  pendingPrompt?: { requestId: string; text: string; attachments: AttachmentSnapshot[]; mentions: Mention[]; queued: boolean; accepted: boolean } | undefined;
  steerInFlight: boolean;
  attention: AttentionKind;
  openedAt: number;
  /** Wall-clock start of the current foreground turn, for the activity view. */
  turnStartedAt: number | null;
  /** When this session last received any event. A turn that says it is running
   *  and has produced nothing for a long time has stopped being a turn. */
  lastEventAt: number | null;
  usage: SessionUsage;
};

export type View =
  | { kind: "welcome" }
  /** The first-run guide, reopened from the command palette. */
  | { kind: "setup" }
  | { kind: "workspace"; workspaceId: string }
  | { kind: "session"; sessionId: string }
  /** A new session whose agent is still starting (see StartingPanel). */
  | { kind: "starting" }
  | { kind: "signin" }
  | { kind: "settings"; tab?: SettingsTab }
  | { kind: "teams" };

/** Settings' tabs, in bar order. Integrations is one of them (⌘I). */
export type SettingsTab = "general" | "chat" | "continuity" | "models" | "notifications" | "integrations" | "phone" | "data";

export type SessionTab = "transcript" | "files" | "changes" | "terminal" | "agents" | "context" | "artifacts" | "images" | "odyssey" | "ultrareview";

/** Every session view, in bar order. The command palette is the keyboard route
 *  to all of them, including the ones the bar keeps behind a menu or an icon,
 *  so it is built from this list rather than a second hand-written one. */
export type OdysseyRuntime = {
  resumeAt: number | null;
  lastReason: string;
  /** When `lastReason` was given. A reason with no age reads as current even
   *  when it is an hour old, which is exactly how a wedged run hides. */
  lastReasonAt: number;
  ticking: boolean;
  /** When the runner first gave this non-progress reason, or null while it is
   *  acting. `stallNotice` turns it into a warning once it has held too long. */
  stalledSince: number | null;
  /** Set once the stall has been journalled and announced, so it is reported
   *  once per episode rather than once per tick. */
  stallNotified: boolean;
  /** What the engine's spend forecast says about the next turn. */
  forecast?: string | null;
};

export const SESSION_TABS: SessionTab[] = ["transcript", "agents", "odyssey", "terminal", "files", "changes", "context", "artifacts", "images", "ultrareview"];

export const DEFAULT_UI_PREFS: UiPrefs = { enterSends: true, reducedMotion: "system", transcriptPage: 50, followUp: "queue", theme: "dark" };

export type OpenMode = { mode: "new" } | { mode: "resume"; session_id: string };

/** A resume the agent refused because of a session lock; offered for explicit recovery. */
export type LockedResume = { workspaceId: string; sessionId: string; error: DesktopError };

export type SignInRun = {
  terminal?: import("@thingmaker/contracts").TerminalInfo;
  finishing?: boolean;
  runId: string | null;
  provider: Provider;
  /** The account signing in, when it is not the active one. */
  account?: string;
  lines: { stream: string; text: string }[];
  urls: string[];
  state: "starting" | "running" | "success" | "failed" | "cancelled" | "timed_out";
};

type State = {
  /** Every provider this build knows, with its program and sign-in state. */
  providers: ProviderInfo[] | null;
  profiles: ProfileInfo[];
  workspaces: WorkspaceRecord[];
  /** Conversations that belong to no project (see commands/quick.rs). */
  quickChats: QuickChat[];
  inspections: Record<string, WorkspaceInspection>;
  /**
   * What each provider's account last said about itself, in one shape
   * (docs/plans/odyssey-second-orchestrator.md §2.4).
   *
   * The providers do not measure the same way: Codex reports two windows as
   * percentages, Claude reports its status on every turn and, once spent, a
   * refusal with a reset time on it. The runner asks `usageFor(provider)`
   * and never has to know the difference.
   *
   * `null` means nothing says the account is spent, which `usageVerdict`
   * reads as room. It is not a claim that the window is empty.
   */
  usage: Partial<Record<Provider, UsageSnapshot | null>>;
  /** The last quota each provider reported, verbatim, for the strip and Context tab. */
  quotas: Partial<Record<Provider, QuotaSnapshot>>;
  /** Desktop records per workspace (archive and pin flags), keyed by workspace id. */
  records: Record<string, SessionRecord[]>;
  /** Git head per workspace. `null` means checked and not a repository. */
  repoInfo: Record<string, RepositoryInfo | null>;
  /** The live Big Thing goal per session. `null` means read and there is none. */
  odyssey: Record<string, OdysseyView | null>;
  /** Runner bookkeeping that is not worth persisting: the resume time a wait
   *  is counting down to, the last reason a tick did nothing and when it was
   *  given, and how long the runner has been unable to act. */
  odysseyRuntime: Record<string, OdysseyRuntime>;
  /** Changes the user asked for while the goal was running, per session. */
  odysseyAmendments: Record<string, AmendmentRecord[]>;
  /** What the run has written into the workspace, as last read; null when the read failed. */
  odysseyNotes: Record<string, WorkspaceNotes | null>;
  /** Plan changes the agent proposed, newest first. */
  /**
   * Sessions a goal has moved *off*, and where it went.
   *
   * Without this the session left behind shows the "set a goal" form, which
   * reads as the run having been wiped rather than moved — the first thing a
   * user said on seeing it was that the session had corrupted. A pointer is
   * the truth and it is one click from the run.
   */
  odysseyMovedAway: Record<string, { goalId: string; title: string; to: string }>;
  odysseyPlanChanges: Record<string, PlanChangeRecord[]>;
  /** Decisions the agent handed to the user, newest first. */
  odysseyQuestions: Record<string, QuestionRecord[]>;
  expandedArchives: Record<string, boolean>;
  view: View;
  sessions: Record<string, LiveSession>;
  sessionOrder: string[];
  /** A side session that could not be copied (a provider with no fork)
   *  starts empty: the summary of its main conversation rides in front of
   *  its first prompt. By the side session's handle. */
  sideBrief: Record<string, string>;
  /** Each live session's team, as the host holds it. */
  teams: Record<string, Combo>;
  /** Jobs per orchestrator session (by attachment handle), oldest first. */
  jobs: Record<string, JobView[]>;
  /** Each live session's agent chat, as the host last reported it. */
  teamChat: Record<string, TeamChatView>;
  /** Reads a session's agent chat again; false when the host could not say. */
  loadTeamChat: (sessionId: string) => Promise<boolean>;
  /** Tokens of prompts shaped like an agent's message that the host has no
   *  receipt for in that session: the person's own text, shown as theirs. */
  teamChatUnknown: Record<string, string[]>;
  /** Asks the host whether it wrote the prompt with this token into the session. */
  checkTeamChat: (sessionId: string, token: string) => Promise<void>;
  /** Ultra-Reviews per session, by the agent's id for it, newest first. */
  ultraReviews: Record<string, UltraReview[]>;
  loadUltraReviews: (host: string) => Promise<void>;
  /** A review changed (the host's event, or a command's reply). */
  noteUltraReview: (review: UltraReview) => void;
  startUltraReview: (request: UrStartRequest) => Promise<boolean>;
  cancelUltraReview: (id: string) => Promise<void>;
  resumeUltraReview: (id: string) => Promise<void>;
  noteOnFinding: (id: string, finding: string, text: string) => Promise<boolean>;
  decideFinding: (id: string, finding: string, decision: UrDecision | null) => Promise<void>;
  deleteUltraReview: (review: UltraReview) => Promise<void>;
  /** The team a new session starts with. */
  defaultCombo: Combo;
  /** Model catalogs per provider, loaded on demand for the team picker. */
  providerModels: Partial<Record<Provider, ProviderModel[]>>;
  /** Saved teams (orchestrator and workers), in name order. */
  teamPresets: TeamPreset[];
  /** Saved helpers, to add to any team. */
  helperPresets: HelperPreset[];
  /** The preset a new session starts with, when one is chosen. */
  defaultPresetId: string | null;
  /** The saved team to use for each project's new sessions. */
  projectTeamPresetIds: Record<string, string>;
  setProjectTeamPreset: (workspaceId: string, presetId: string | null) => Promise<void>;
  drafts: Record<string, string>;
  /** The provider sign-in in progress, if any. */
  signIn: SignInRun | null;
  settings: NotificationSettings | null;
  closeRequest: { activity: { id: string; active: boolean; detachedCalls: number }[] } | null;
  error: DesktopError | null;
  busy: string | null;
  focusComposerToken: number;
  lockedResume: LockedResume | null;
  /** Uncertain submissions per workspace, loaded on demand (REC-01). */
  outbox: Record<string, OutboxEntry[]>;
  /** Snapshotted media per session, sent with the next prompt (UX-06). */
  attachments: Record<string, AttachmentSnapshot[]>;
  /** File mentions per session with explicit delivery mode (UX-07). */
  mentions: Record<string, Mention[]>;
  contextBundles: Record<string, ContextBundle[]>;
  paletteOpen: boolean;
  sessionTab: SessionTab;
  /** Places visited, oldest first, and where in them the user is now. */
  navHistory: NavEntry[];
  navIndex: number;
  /** A milestone the Big Thing tab should bring into view, asked for from
   *  the transcript, and the transcript card asked for from the board. Each
   *  is taken by the view that shows it. */
  odysseyFocus: { sessionId: string; milestone: number } | null;
  transcriptFocus: { sessionId: string; cardKey: string } | null;
  /** A diff scope the Changes tab should switch to, asked for from the
   *  transcript's code-changes card; taken by the tab. */
  changesFocus: DiffScope | null;
  transcriptSearchOpen: boolean;
  /** Text for the screen-reader live region (settled events only, UX-11). */
  announcement: string;
  uiPrefs: UiPrefs;
  appUpdate: AppUpdateState;
  /** Asks GitHub for a newer release. At startup it may raise the popup. */
  checkForUpdate: (reason: "startup" | "timer" | "manual") => Promise<void>;
  /** Downloads, verifies and installs the update found, then restarts. */
  installUpdate: () => Promise<void>;
  /** Closes the startup popup; `forever` also turns the reminder off. */
  dismissUpdatePrompt: (forever?: boolean) => void;
  sidebarCollapsed: boolean;
  /** Model/effort remembered from the last picker change (new sessions). */
  sessionDefaults: SessionDefaults;
  /** Images seen appearing under output/imagegen per session (observed files). */
  observedImages: Record<string, string[]>;
  seenImageFiles: Record<string, string[]>;

  bootstrap: () => Promise<void>;
  rehydrate: () => Promise<void>;
  loadOutbox: (workspaceId: string) => Promise<void>;
  discardOutbox: (workspaceId: string, requestId: string) => Promise<void>;
  resendOutbox: (workspaceId: string, entry: OutboxEntry) => Promise<void>;
  dismissLockedResume: () => void;
  addAttachments: (sessionId: string, snapshots: AttachmentSnapshot[]) => void;
  removeAttachment: (sessionId: string, id: string) => void;
  addMention: (sessionId: string, mention: Mention) => void;
  updateMention: (sessionId: string, index: number, mention: Mention) => void;
  removeMention: (sessionId: string, index: number) => void;
  loadContextBundles: (workspaceId: string) => Promise<void>;
  saveContextBundle: (workspaceId: string, bundle: ContextBundle) => Promise<void>;
  deleteContextBundle: (workspaceId: string, id: string) => Promise<void>;
  applyContextBundle: (sessionId: string, bundle: ContextBundle) => void;
  setPaletteOpen: (open: boolean) => void;
  setSessionTab: (tab: SessionTab) => void;
  /** Open the Big Thing tab, at a milestone (0-based) when one is given. */
  openBigThingAt: (sessionId: string, milestone: number | null) => void;
  /** Open the conversation at one of its cards. */
  showInTranscript: (sessionId: string, cardKey: string) => void;
  clearJumpFocus: () => void;
  /** A document on its way to a session's Big Thing tab, by absolute path. */
  bigThingDoc: Record<string, string>;
  /** Opens Big Thing with this document: a new goal planned from it, or a change to the running one. */
  sendDocToBigThing: (sessionId: string, path: string) => void;
  /** The document waiting for this session's Big Thing tab, taken once. */
  takeBigThingDoc: (sessionId: string) => string | null;
  setTranscriptSearchOpen: (open: boolean) => void;
  announce: (text: string) => void;
  toggleSidebar: () => void;
  removeWorkspace: (workspaceId: string) => Promise<void>;
  loadQuickChats: () => Promise<void>;
  /** A new quick chat, opened on its start page. */
  newQuickChat: () => Promise<void>;
  /** Makes a quick chat temporary (deleted when ThingMaker quits), or not. */
  setQuickChatTemporary: (workspaceId: string, temporary: boolean) => Promise<void>;
  /** Deletes a quick chat for good, after asking unless `confirmed`. */
  deleteQuickChat: (workspaceId: string, confirmed?: boolean) => Promise<boolean>;
  /** Deletes for good every Claude and Codex session ThingMaker ran in a project, from this computer. */
  wipeSessions: (workspaceId: string) => Promise<WipeReport | null>;
  /** Renames the project's folder on disk and everything that names it; null on failure (the error is shown). */
  renameWorkspaceFolder: (workspaceId: string, newName: string) => Promise<RenameReport | null>;
  resetSessionDefaults: () => Promise<void>;
  /** Sets the model or effort new sessions on `provider` start with; an empty value goes back to the account's. */
  setSessionDefault: (provider: Provider, patch: { model?: string; reasoningEffort?: string }) => Promise<void>;
  noteImageFiles: (sessionId: string, files: string[], initial: boolean) => void;
  loadUiPrefs: () => Promise<void>;
  setUiPrefs: (patch: Partial<UiPrefs>) => Promise<void>;
  exportTranscript: (sessionId: string) => Promise<void>;
  exportContinuity: (sessionId: string) => Promise<void>;
  loadSettings: () => Promise<void>;
  saveSettings: (settings: NotificationSettings) => Promise<void>;
  testNotification: () => Promise<void>;
  dismissCloseRequest: () => void;
  hideToTray: () => Promise<void>;
  quitAndStop: () => Promise<void>;
  setView: (view: View) => void;
  /** Step back or forward through `navHistory` (⌘[ and ⌘]). */
  navigate: (direction: -1 | 1) => void;
  /** Adds a folder the person picks, and opens it unless `stay` (the first-run guide keeps its place). Returns the workspace's id, or null when none was added. */
  addWorkspaceByPicker: (options?: { stay?: boolean }) => Promise<string | null>;
  addWorkspaceByPath: (path: string, options?: { stay?: boolean }) => Promise<string | null>;
  selectWorkspace: (workspaceId: string) => Promise<void>;
  inspectWorkspace: (workspaceId: string) => Promise<WorkspaceInspection | null>;
  trust: (workspaceId: string, state: TrustState) => Promise<void>;
  loadProviders: () => Promise<void>;
  /** A session's kept title changed in the host. */
  noteSessionTitle: (change: SessionTitleChange) => void;
  /** Installed and newest version of each provider program; null until asked. */
  providerVersions: Partial<Record<Provider, ProviderVersion>> | null;
  /** The provider whose program is updating now, and how the last update of each went. */
  providerUpdate: { running: Provider | null; results: Partial<Record<Provider, { ok: boolean; text: string }>> };
  loadProviderVersions: (askLatest?: boolean) => Promise<void>;
  updateProvider: (provider: Provider) => Promise<void>;
  /**
   * Re-reads one provider's account now. Codex is asked; Claude's refusal is
   * re-read against its clock, because nothing on that side answers the
   * question in between turns. Returns the resulting snapshot.
   */
  refreshUsage: (provider: Provider) => Promise<UsageSnapshot | null>;
  /** Records what a provider reported about its quota. */
  noteQuota: (quota: QuotaSnapshot) => void;
  loadTeam: (sessionId: string) => Promise<void>;
  /** Changes a live session's team; the next `delegate` uses it. */
  saveTeam: (sessionId: string, combo: Combo) => Promise<void>;
  loadDefaultCombo: () => Promise<void>;
  saveDefaultCombo: (combo: Combo) => Promise<void>;
  /** A provider's models, asked once; `fresh` asks again (Local Models change when a model is downloaded). */
  loadProviderModels: (provider: Provider, fresh?: boolean) => Promise<ProviderModel[]>;
  noteJob: (job: JobView) => void;
  cancelJob: (sessionId: string, jobId: string) => Promise<void>;
  /** Ends a waiting job's wait now. */
  retryJob: (sessionId: string, jobId: string) => Promise<void>;
  /** Shows a job's worker session, attaching to it if this window has not. */
  openWorker: (job: JobView) => Promise<void>;
  /**
   * Moves the work to a new session on another provider with the same team,
   * briefed from this one. Models cannot share a context window, so the brief
   * is put in the new session's composer to read and send.
   */
  handOff: (sessionId: string, provider: Provider, preset?: TeamPreset) => Promise<void>;
  /** Prepares an editable continuation after exhaustion; never sends a prompt. */
  prepareQuotaContinuation: (sessionId: string, target: { provider: Provider; accountId?: string }) => Promise<string | null>;
  /**
   * Copies a session into a new one nested under it, on the same provider,
   * model, effort and team, and opens it. With a point, the copy ends there
   * ("Fork from here"). Returns the fork's handle.
   */
  forkSession: (source: ForkSource, point?: ForkPoint, options?: { worktree?: boolean; side?: boolean }) => Promise<string | null>;
  /** What nested forks reported, waiting for a session's next message, by
   *  the parent's agent session id. Sent with that message, as context. */
  nestedReports: Record<string, NestedReport[]>;
  /** Whether a fork passes its answers on to its parent, by its agent session id. */
  reporting: Record<string, boolean>;
  loadNesting: (workspaceId: string, agentSessionId: string) => Promise<void>;
  setReporting: (workspaceId: string, agentSessionId: string, on: boolean) => Promise<void>;
  /** Lists a nested session on its own again, or under another session. */
  nestSession: (workspaceId: string, agentSessionId: string, parentAgentSessionId: string | null) => Promise<void>;
  /**
   * The provider (and model, effort) a session's next message goes to,
   * chosen in the model menu; nothing changes until that message is sent.
   * Choosing the session's own provider again clears it.
   */
  pendingProvider: Record<string, PendingProvider>;
  setPendingProvider: (sessionId: string, pending: PendingProvider | null) => void;
  /** Sessions that continue another on a different provider, by agent session id. */
  continuations: Record<string, Continuation>;
  /** The earlier session's transcript, kept for the session that continues it (this run only). */
  continuationHistory: Record<string, Card[]>;
  loadContinuations: () => Promise<void>;
  /**
   * Sends the draft to a new session on the pending provider, briefed from
   * this one without showing the brief, and carries the conversation on there.
   */
  continueOnProvider: (sessionId: string) => Promise<void>;
  loadTeamPresets: () => Promise<void>;
  saveTeamPreset: (preset: TeamPreset) => Promise<void>;
  deleteTeamPreset: (id: string) => Promise<void>;
  loadHelperPresets: () => Promise<void>;
  /** Saves a team's helper as a preset (updating the one with its name); null when it failed. */
  saveHelperPreset: (worker: WorkerSlot) => Promise<HelperPreset | null>;
  deleteHelperPreset: (id: string) => Promise<void>;
  /** Saves helpers read from a file, all at once. */
  addHelperPresets: (presets: HelperPreset[]) => Promise<boolean>;
  /** Saves teams read from a file, all at once. */
  addTeamPresets: (presets: TeamPreset[]) => Promise<boolean>;
  /** Makes a preset (or none) what a new session starts with. */
  setDefaultPreset: (id: string | null) => Promise<void>;
  /** Starts a session led by the preset's orchestrator, with its workers. */
  openSessionWithPreset: (workspaceId: string, preset: TeamPreset, message?: string, attachments?: AttachmentSnapshot[]) => Promise<void>;
  /**
   * Applies a preset to a live session. Workers change at once; the
   * orchestrator's model and effort are set through its own options. A
   * preset led by another provider is a handoff, and that is returned so
   * the caller can say so rather than doing it silently.
   */
  applyPresetToSession: (sessionId: string, preset: TeamPreset) => Promise<"applied" | "needs_handoff">;
  sampleUsage: (sessionId: string, phase: "start" | "tick" | "settle") => Promise<void>;
  loadRecords: (workspaceId: string) => Promise<void>;
  archiveSession: (workspaceId: string, agentSessionId: string, archived: boolean) => Promise<void>;
  /** Archives a session, stopping it first when it is open. Its transcript
   *  stays on disk, and Archived can bring it back. */
  stopAndArchive: (workspaceId: string, agentSessionId: string, liveId?: string) => Promise<void>;
  pinSession: (workspaceId: string, agentSessionId: string, pinned: boolean) => Promise<void>;
  renameSession: (workspaceId: string, agentSessionId: string, title: string | null) => Promise<void>;
  loadRepoInfo: (workspaceId: string) => Promise<void>;
  loadOdyssey: (sessionId: string) => Promise<void>;
  createOdyssey: (sessionId: string, request: NewOdyssey) => Promise<void>;
  editOdysseyGoal: (sessionId: string, id: string, edit: GoalEdit) => Promise<void>;
  deleteOdyssey: (sessionId: string, id: string) => Promise<void>;
  addMilestone: (sessionId: string, request: { odysseyId: string; title: string; detail?: string; checkKind?: string; checkSpec?: string | null }) => Promise<void>;
  editMilestone: (sessionId: string, id: string, edit: MilestoneEdit) => Promise<void>;
  reorderMilestones: (sessionId: string, odysseyId: string, orderedIds: string[]) => Promise<void>;
  deleteMilestone: (sessionId: string, id: string) => Promise<void>;
  addStep: (sessionId: string, milestoneId: string, title: string, detail?: string, dependsOn?: string[]) => Promise<void>;
  deleteStep: (sessionId: string, id: string) => Promise<void>;
  /** A human moving a task by hand; the record stamps the move. */
  setStepState: (sessionId: string, id: string, state: StepState) => Promise<void>;
  /** Queues an amendment asking the agent to break a milestone into tasks. */
  odysseyRequestTasks: (sessionId: string, milestoneIndex: number) => Promise<void>;
  odysseyLoadInbox: (sessionId: string, odysseyId: string) => Promise<void>;
  /** Applies or rejects a held plan change; either way the agent is told. */
  odysseyDecidePlanChange: (sessionId: string, id: string, decision: "apply" | "reject", note?: string) => Promise<void>;
  /** Answers a question the agent asked, or dismisses it; the agent is told either way. */
  odysseyAnswerQuestion: (sessionId: string, id: string, answer: string | null) => Promise<void>;
  /** The user's decision at one of their approvals, or at a loop's limit. */
  odysseyDecideApproval: (sessionId: string, id: string, decision: ApprovalDecision, comment: string | null) => Promise<void>;
  /** Changes a task: its kind, the results it uses, its loop. */
  odysseyEditStep: (sessionId: string, stepId: string, edit: StepEdit) => Promise<void>;
  refreshOdyssey: (sessionId: string, id: string) => Promise<void>;
  odysseyStart: (sessionId: string) => Promise<void>;
  /** Points the goal at another open session, or a fresh one of either agent. */
  odysseyMoveTo: (sessionId: string, target: MoveTarget) => Promise<void>;
  odysseyPause: (sessionId: string, reason?: string) => Promise<void>;
  /** Asks the engine to look at the goal now. */
  odysseyTick: (sessionId: string) => Promise<void>;
  /** What the engine tells the interface. */
  noteBigThing: (event: BigThingEvent) => void;
  odysseyVerifyManually: (sessionId: string, milestoneId: string) => Promise<void>;
  odysseyRunCheck: (sessionId: string, milestoneId: string) => Promise<void>;
  odysseyRequestPlan: (sessionId: string) => Promise<void>;
  odysseyAddAmendment: (sessionId: string, request: NewAmendment) => Promise<void>;
  odysseyDiscardAmendment: (sessionId: string, id: string) => Promise<void>;
  odysseyLoadAmendments: (sessionId: string, odysseyId: string) => Promise<void>;
  /** Closes activity the runtime never reported finishing. Presentation only. */
  clearStaleActivity: (sessionId: string) => void;
  /** Re-reads the handoff and subagent notes for the screen. */
  odysseyRefreshNotes: (sessionId: string) => Promise<void>;
  /** Re-reads the account behind a session after a quota-shaped failure. */
  resampleAccount: (sessionId: string, message?: string | null) => Promise<void>;
  /** Which agent a live session runs, for the account its turns are charged to. */
  sessionAgent: (sessionId: string) => Provider;
  /** The account snapshot that governs this session, by its agent. */
  usageFor: (provider: Provider) => UsageSnapshot | null;
  /** Records what a Claude turn's failure said about the account. */
  noteClaudeLimit: (message: string | null | undefined) => boolean;
  /** Asks the Claude account whether it has room, without spending a turn. */
  refreshClaudeUsage: () => Promise<UsageSnapshot | null>;
  toggleArchiveList: (workspaceId: string) => void;
  /** Opens or resumes a session. A resume runs on the provider that wrote it; a new one on `provider`, else the last one used. */
  openSession: (workspaceId: string, mode: OpenMode, provider?: Provider, launch?: NewSessionLaunch) => Promise<void>;
  /** Opens a fresh session on a named agent and returns how to address it. */
  openSessionFor: (workspaceId: string, provider: Provider, combo?: Combo) => Promise<{ key: string; agentSessionId: string } | null>;
  selectSession: (sessionId: string) => void;
  /** Opens the main session's side session: the one it has, resumed, or a
   *  new copy of the main conversation when `fresh` or there is none. */
  setDraft: (sessionId: string, draft: string) => void;
  send: (sessionId: string) => Promise<void>;
  steer: (sessionId: string) => Promise<void>;
  /** Holds the composer's prompt for after the running turn. */
  enqueue: (sessionId: string) => Promise<void>;
  queueOp: (sessionId: string, op: QueueOp) => Promise<void>;
  /** Takes a queued prompt back into the composer to change it. */
  editQueued: (sessionId: string, id: string) => Promise<void>;
  cancel: (sessionId: string) => Promise<void>;
  stop: (sessionId: string) => Promise<void>;
  refreshSnapshot: (sessionId: string) => Promise<void>;
  setConfigOption: (sessionId: string, configId: string, value: string) => Promise<void>;
  /** A session whose agent is still starting — a new one, or one being
   *  resumed (`resuming`, with its known title) — shown at once by
   *  StartingPanel: what the person writes meanwhile, and whether they sent
   *  it (it goes out once the session is ready). */
  starting: { id: string; workspaceId: string; provider: Provider; draft: string; queued: boolean; error?: string; resuming?: boolean; title?: string } | null;
  setStartingDraft: (draft: string, queued: boolean) => void;
  /** Signs in to a provider: its active account, or `account`. */
  startSignIn: (provider: Provider, account?: string) => Promise<void>;
  /** Each provider's accounts (Claude Code, Codex), the default first. */
  accounts: Partial<Record<Provider, AccountView[]>>;
  loadAccounts: (provider: Provider) => Promise<void>;
  /** Adds an account and starts its sign-in. */
  addAccount: (provider: Provider, label: string) => Promise<void>;
  renameAccount: (provider: Provider, id: string, label: string) => Promise<void>;
  removeAccount: (provider: Provider, id: string) => Promise<void>;
  /**
   * Makes an account the active one: new sessions, status and usage use it.
   * With `sessionId`, that session's conversation moves to it too, when it
   * is on this provider and idle: it is stopped and resumed under the
   * account, which shares its history.
   */
  setActiveAccount: (provider: Provider, id: string, sessionId?: string) => Promise<void>;
  cancelSignIn: () => Promise<void>;
  finishGeminiSignIn: () => Promise<void>;
  openUrl: (url: string) => Promise<void>;
  focusComposer: () => void;
  clearError: () => void;
  setError: (error: unknown) => void;
};

function asError(error: unknown): DesktopError {
  if (typeof error === "object" && error !== null && "code" in error && "message" in error) {
    return error as DesktopError;
  }
  return { code: "IO", message: error instanceof Error ? error.message : String(error), retry: "user_action" };
}

function promptPublished(projection: Projection, requestId: string): boolean {
  return projection.messages.has(`user|prompt-${requestId}`) || projection.queue.items.some((item) => item.id === requestId);
}

/** Restore a failed submission without overwriting a newly started draft. */
function restorePrompt(state: State, sessionId: string, prompt: NonNullable<LiveSession["pendingPrompt"]>) {
  const current = state.drafts[sessionId] ?? "";
  const attachments = state.attachments[sessionId] ?? [];
  const mentions = state.mentions[sessionId] ?? [];
  return {
    drafts: { ...state.drafts, [sessionId]: [prompt.text, current].filter(Boolean).join("\n\n") },
    attachments: { ...state.attachments, [sessionId]: [...prompt.attachments.filter((a) => !attachments.some((b) => a.id === b.id)), ...attachments] },
    mentions: { ...state.mentions, [sessionId]: [...prompt.mentions.filter((a) => !mentions.some((b) => JSON.stringify(a) === JSON.stringify(b))), ...mentions] },
  };
}

function attentionFor(event: EventEnvelope, current: AttentionKind): AttentionKind {
  const payload = event.payload;
  // Permission requests are answered by the workspace's trust, with nobody
  // asked, so they need no input from anyone.
  if (payload.type === "exited") return "failed";
  if (payload.type === "turn" && payload.effect === "settled" && payload.kind === "foreground") {
    if (current === "needs_input") return current;
    return payload.phase === "succeeded" ? "completed" : "failed";
  }
  return current;
}

/** Whether a live session is a worker a delegation opened. */
function isWorkerSession(get: Get, id: string, session: LiveSession): boolean {
  return isWorkerIn(get(), id, session);
}

/** The same, for a component's selector: a worker reports to its orchestrator. */
export function isWorkerIn(state: Pick<State, "jobs" | "records">, id: string, session: LiveSession | undefined): boolean {
  if (Object.values(state.jobs).some((jobs) => jobs.some((job) => job.workerSession === id))) return true;
  const agentId = session?.snapshot.agentSessionId;
  // Listed under another session and not a fork of it: a worker.
  const record = agentId ? (state.records[session?.workspaceId ?? ""] ?? []).find((entry) => entry.agentSessionId === agentId) : undefined;
  return !!session && !!record?.parentSessionId && record.origin !== "fork" && record.origin !== "side";
}


export function basenameOf(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** A session's name: the desktop's rename (or a worker's job title) first, then the agent's, then its first prompt. */
/**
 * What a live session is called: the person's rename, then the title kept in
 * its record (the provider's own, or its first prompt; it outlives the
 * session), then what the stream says now. Records default to the store's,
 * so every caller names a session the same way.
 */
/** Where a session's next message goes when it is another provider. */
export type PendingProvider = { resources?: WorkerSlot["resources"]; provider: Provider; model?: string; effort?: string };
/** The session a fork is copied from: open (`key`) or only in the list. */
export type ForkSource = { workspaceId: string; agentSessionId: string; provider: Provider; key?: string };

/**
 * A session that carries on another's conversation on a different provider.
 * `briefPending` says a prepared brief is waiting for the session's first
 * message; the brief itself is kept apart (see `pendingBriefKey`).
 * `pendingBrief` is how development builds kept it, and is never written.
 */
export type Continuation = { fromAgentSessionId: string; fromProvider: Provider; briefPending?: boolean; pendingBrief?: string; archive?: ContinuityArchive; target?: { workspaceId: string; provider: Provider; accountId: string } };

/** A durable id can coincide across accounts/providers. Never inject another
 * account's pending text or show its private archive location. */
export function sessionContinuation(session: LiveSession | undefined, continuations: Record<string, Continuation>): Continuation | undefined {
  const entry = session?.snapshot.agentSessionId ? continuations[session.snapshot.agentSessionId] : undefined;
  if (!entry || !session) return undefined;
  // Older records have no trustworthy account/workspace provenance. A link
  // without private context is safe to keep; pending text needs a fresh handoff.
  if (!entry.target) return entry.archive || entry.pendingBrief || entry.briefPending ? undefined : entry;
  if (entry.target.workspaceId !== session.workspaceId || entry.target.provider !== session.snapshot.provider) return undefined;
  if (entry.target.accountId === (session.snapshot.accountId ?? "default")) return entry;
  // The same conversation, moved to another account: the link back to the
  // earlier one is still its own. What was prepared for the first account
  // (the archive, a waiting brief) stays with that account.
  return { fromAgentSessionId: entry.fromAgentSessionId, fromProvider: entry.fromProvider };
}

function continuationTarget(session: LiveSession) {
  return { workspaceId: session.workspaceId, provider: session.snapshot.provider, accountId: session.snapshot.accountId ?? "default" };
}

const CONTINUATIONS_KEY = "session_continuations";
/** Continuations kept: years of them, and well inside what one preference may hold. */
const CONTINUATIONS_KEPT = 300;
const quotaContinuationSessions = new Set<string>();
const quotaContinuationProviders = new Set<Provider>();
const leaderContinuations = new Set<string>();
let continuationWrites: Promise<unknown> = Promise.resolve();

/**
 * Records that a session carries on another's. The record is a convenience
 * (the link back, the archive's notice), kept in one preference with every
 * other continuation: a write that fails, as one over the preference size
 * limit does, must not undo a change of leader that has otherwise worked. So
 * the record is kept in memory at once and the write is not waited on to
 * succeed.
 */
function saveContinuation(get: Get, set: Set, agent: string, entry: Continuation): Promise<void> {
  const { [agent]: _replaced, ...others } = get().continuations;
  // Newest last; the oldest go once there are more than are kept.
  const kept = Object.entries({ ...others, [agent]: entry }).slice(-CONTINUATIONS_KEPT);
  set({ continuations: Object.fromEntries(kept) });
  const write = continuationWrites.catch(() => undefined).then(() => api.prefSet(CONTINUATIONS_KEY, get().continuations)).then(() => undefined, () => undefined);
  continuationWrites = write;
  return write;
}

/**
 * Where a prepared continuation's brief waits until its first message is
 * sent: a preference of its own per session. A brief runs to tens of
 * kilobytes, and kept inside the map of continuations a handful of unsent
 * ones filled the 256 KiB a preference may hold, after which no continuation
 * could be recorded at all.
 */
const pendingBriefKey = (agent: string) => `continuation_brief:${agent}`;

/** Keeps a prepared brief for after a restart. Losing it costs only that. */
async function keepPendingBrief(agent: string, brief: string): Promise<void> {
  await api.prefSet(pendingBriefKey(agent), brief).catch(() => undefined);
}

/** Puts a waiting brief back in front of a continuation's first message. */
async function restorePendingBrief(get: Get, set: Set, sessionId: string): Promise<void> {
  const waiting = () => {
    const session = get().sessions[sessionId];
    const entry = sessionContinuation(session, get().continuations);
    return entry?.briefPending && !get().sideBrief[sessionId] ? session?.snapshot.agentSessionId ?? null : null;
  };
  const agent = waiting();
  if (!agent) return;
  const brief = await api.prefGet<string>(pendingBriefKey(agent)).catch(() => null);
  // Sent, or replaced, while it was being read.
  if (typeof brief !== "string" || !brief || waiting() !== agent) return;
  set({ sideBrief: { ...get().sideBrief, [sessionId]: brief } });
}

/** Freeze the exposed conversation, then recover retained events still waiting
 * for the renderer. Never put drafts or queued prompts in the document. */
async function continuityDocument(get: Get, session: LiveSession): Promise<{ markdown: string; sequence: string }> {
  const projection = structuredClone(session.projection);
  const snapshot = await api.sessionSnapshot(session.handle);
  if (!snapshot.agentSessionId || snapshot.agentSessionId !== session.snapshot.agentSessionId) throw new Error("Cannot read continuity state without a stable native session ID.");
  const [jobs, goal] = await Promise.all([
    api.delegationJobs(session.handle),
    api.odysseyForSession(session.workspaceId, snapshot.agentSessionId),
  ]);
  if (BigInt(snapshot.lastSequence) > BigInt(projection.lastSequence)) {
    const events = await api.sessionHistory(session.handle, projection.lastSequence);
    for (const event of events) {
      if (BigInt(event.sequence) > BigInt(snapshot.lastSequence)) break;
      if (BigInt(event.sequence) > BigInt(projection.lastSequence)) applyEvent(projection, event);
    }
  }
  const workspace = get().workspaces.find((entry) => entry.id === session.workspaceId);
  const warnings: string[] = [];
  if (snapshot.historyDropped > 0) warnings.push(`The native replay buffer has dropped ${snapshot.historyDropped} earlier events. Visible history already cached by ThingMaker is included, but earlier unavailable content cannot be recovered or guaranteed complete.`);
  if (projection.snapshotNeeded) warnings.push("The conversation cache reported a history gap; some earlier content may be unavailable.");
  if (BigInt(projection.lastSequence) < BigInt(snapshot.lastSequence)) warnings.push("Some events up to the snapshot were unavailable for replay. This document may be incomplete.");
  const markdown = continuityMarkdown({
    title: sessionTitle(session), provider: snapshot.provider, agentSessionId: snapshot.agentSessionId,
    workspacePath: workspace?.displayPath ?? workspace?.canonicalRoot ?? session.workspaceId,
    projection, ...(get().teams[session.handle.id] ? { team: get().teams[session.handle.id]! } : {}), goal, jobs,
  });
  return { markdown: warnings.length ? `> Context availability warnings\n> ${warnings.join("\n> ")}\n\n${markdown}` : markdown, sequence: snapshot.lastSequence };
}

/** What a change of leader carries from the session being left. */
type HandoffContext = { excerpt: string; archive?: ContinuityArchive; sequence?: string };

/**
 * The recent excerpt and, when archives are kept, the session's visible
 * conversation saved for the new leader to consult.
 *
 * The archive is an extra. When it cannot be made (the conversation is over
 * the archive's size limit, the storage refused, the session has no durable
 * id yet) the excerpt goes alone, as it did before archives existed, rather
 * than the change of leader failing. A phone never makes one: archives are
 * written on the computer, by the computer's own interface.
 */
async function handoffContext(get: Get, session: LiveSession, budget = 6000): Promise<HandoffContext> {
  const excerpt = get().sideBrief[session.handle.id] ?? handoffBrief(session.projection.cards, session.snapshot.provider, budget);
  if (get().uiPrefs.autoContinuity === false || IS_REMOTE) return { excerpt };
  try {
    const { markdown, sequence } = await continuityDocument(get, session);
    return { excerpt, archive: await api.continuitySave(session.handle, markdown), sequence };
  } catch {
    return { excerpt };
  }
}

/** Whether a session has the tool an archive is read with. */
function readsArchives(snapshot: Snapshot): boolean {
  return !isLocalChat(snapshot) && snapshot.leadsTeam !== false;
}

/**
 * Binds the archive to the session taking over, and words that session's
 * brief. A session with no way to read one (a local chat, or any session
 * without the team tools) gets the excerpt alone, and so does one the
 * archive could not be bound to: the change of leader goes ahead either way.
 */
async function bindArchive(source: LiveSession, target: LiveSession | undefined, context: HandoffContext): Promise<{ brief: string; archive?: ContinuityArchive }> {
  const { excerpt, archive } = context;
  if (!archive || !target || !readsArchives(target.snapshot)) return { brief: excerpt };
  try {
    await api.continuityAttach(source.handle, target.handle, archive.id);
    return { brief: continuityBrief(excerpt, archive), archive };
  } catch {
    return { brief: excerpt };
  }
}

function idleHandoffSource(get: Get, sessionId: string): LiveSession {
  const source = get().sessions[sessionId];
  if (!source || source.projection.attachment !== "attached" || source.projection.process === "exited") throw { code: "NOT_READY", message: t("The source session changed while the continuation was opening. Try again."), retry: "user_action" };
  const p = source.projection;
  if (source.inFlightRequestId || source.pendingPrompt || source.steerInFlight
    || ["running", "awaiting_user", "cancelling"].includes(p.foreground) || p.autonomous.size
    || [...p.detached.values()].some((value) => value === "active" || value === "cancellation_requested")
    || [...p.background.values()].some((value) => value.state === "running")
    || [...p.inspector.agents.values()].some((value) => value.status === "working" || value.status === "starting")
    || (get().jobs[sessionId] ?? []).some((job) => ["starting", "running", "waiting"].includes(job.status))) throw { code: "NOT_READY", message: t("Wait for this session and its workers to finish before preparing a continuation."), retry: "user_action" };
  if (p.queue.items.length || p.queue.sending) throw { code: "NOT_READY", message: t("Resolve or remove the queued messages before preparing a continuation."), retry: "user_action" };
  const goal = get().odyssey[sessionId]?.goal;
  if (goal && goal.state !== "complete" && goal.state !== "abandoned") throw { code: "NOT_READY", message: t("Move this goal from the Big Thing tab so its progress follows it."), retry: "user_action" };
  return source;
}

async function validateLeaderSource(get: Get, expected: LiveSession, sequence?: string) {
  const check = () => {
    const current = idleHandoffSource(get, expected.handle.id);
    if (current.handle.attachmentGeneration !== expected.handle.attachmentGeneration
      || current.snapshot.agentSessionId !== expected.snapshot.agentSessionId
      || current.snapshot.provider !== expected.snapshot.provider
      || (current.snapshot.accountId ?? "default") !== (expected.snapshot.accountId ?? "default")) throw { code: "NOT_READY", message: t("The source session changed while the continuation was opening. Try again."), retry: "user_action" };
  };
  check();
  const [snapshot, jobs, goal] = await Promise.all([
    api.sessionSnapshot(expected.handle), api.delegationJobs(expected.handle),
    expected.snapshot.agentSessionId ? api.odysseyForSession(expected.workspaceId, expected.snapshot.agentSessionId) : Promise.resolve(null),
  ]);
  check();
  if ((sequence !== undefined && snapshot.lastSequence !== sequence)
    || snapshot.agentSessionId !== expected.snapshot.agentSessionId || snapshot.provider !== expected.snapshot.provider
    || (snapshot.accountId ?? "default") !== (expected.snapshot.accountId ?? "default")) throw { code: "NOT_READY", message: t("The source session changed while the continuation was opening. Try again."), retry: "user_action" };
  if (["running", "awaiting_user", "cancelling"].includes(snapshot.foreground) || snapshot.autonomousTurns.length
    || Object.values(snapshot.detachedCalls).some((value) => value === "active" || value === "cancellation_requested")
    || snapshot.backgroundTasks?.some((value) => value.state === "running")
    || jobs.some((job) => ["starting", "running", "waiting"].includes(job.status))) throw { code: "NOT_READY", message: t("Wait for this session and its workers to finish before preparing a continuation."), retry: "user_action" };
  if (snapshot.queue?.items.length || snapshot.queue?.sending) throw { code: "NOT_READY", message: t("Resolve or remove the queued messages before preparing a continuation."), retry: "user_action" };
  if (goal && goal.goal.state !== "complete" && goal.goal.state !== "abandoned") throw { code: "NOT_READY", message: t("Move this goal from the Big Thing tab so its progress follows it."), retry: "user_action" };
}

async function discardContinuation(get: Get, set: Set, handle: SessionHandle) {
  try {
    await api.sessionStop(handle);
    const { [handle.id]: _unused, ...sessions } = get().sessions;
    const { [handle.id]: _brief, ...sideBrief } = get().sideBrief;
    set({ sessions, sideBrief, sessionOrder: get().sessionOrder.filter((key) => key !== handle.id) });
  } catch { /* Keep a target that could not be stopped visible. */ }
}

function consumeContinuationBrief(get: Get, set: Set, sessionId: string) {
  const agent = get().sessions[sessionId]?.snapshot.agentSessionId;
  const entry = sessionContinuation(get().sessions[sessionId], get().continuations);
  if (!agent || !entry?.briefPending) return;
  const { briefPending: _sent, ...history } = entry;
  void saveContinuation(get, set, agent, history);
  void api.prefSet(pendingBriefKey(agent), null).catch(() => undefined);
}

export function sessionTitle(session: LiveSession, records?: SessionRecord[]): string {
  const id = session.snapshot.agentSessionId;
  const record = id ? (records ?? useStore.getState().records[session.workspaceId])?.find((entry) => entry.agentSessionId === id) : undefined;
  if (record?.titleOverlay) return record.titleOverlay;
  if (record?.autoTitle) return record.autoTitle;
  if (session.projection.title) return session.projection.title;
  for (const card of session.projection.cards) {
    if (card.kind === "message" && card.message.role === "user") {
      const text = card.message.blocks.find((b) => b.type === "text");
      if (text && text.type === "text" && withoutHandoff(text.text).trim()) return withoutHandoff(text.text).trim().slice(0, 60);
    }
  }
  return t("Session {id}", { id: session.handle.id.replace(/^kw-/, "").slice(0, 8) });
}

/** What a screen reader hears when a foreground turn settles. */
function turnAnnouncement(title: string, phase: TurnPhase): string {
  switch (phase) {
    case "succeeded":
      return t("{title}: turn succeeded", { title });
    case "failed":
      return t("{title}: turn failed", { title });
    case "cancelled":
      return t("{title}: turn cancelled", { title });
    default:
      return t("{title}: turn {phase}", { title, phase });
  }
}

/** Difference of one window between two samples; a changed reset time means the window rolled over. */
export function windowDelta(before: UsageSnapshot["primary"] | undefined, after: UsageSnapshot["primary"] | undefined): WindowDelta | null {
  if (!before || !after) return null;
  const reset = before.resetAtUnix !== undefined && after.resetAtUnix !== undefined && before.resetAtUnix !== after.resetAtUnix;
  return { before: before.usedPercent, after: after.usedPercent, delta: reset ? null : after.usedPercent - before.usedPercent, reset };
}

/**
 * A provider's quota report as the runner's usage snapshot.
 *
 * Windows the provider reports as percentages become `primary`/`secondary`,
 * shortest first. A report with no percentage at all (Claude says only
 * "allowed" until it is spent) is no snapshot, except when it is a refusal:
 * then the reset it names is the whole reading.
 */
export function usageFromQuota(quota: QuotaSnapshot): UsageSnapshot | null {
  const measured = quota.windows
    .filter((window) => window.usedPercent !== undefined)
    .sort((a, b) => (a.windowMinutes ?? Number.MAX_SAFE_INTEGER) - (b.windowMinutes ?? Number.MAX_SAFE_INTEGER));
  const toWindow = (window: QuotaSnapshot["windows"][number]) => ({
    ...(quota.provider === "gemini" ? { label: window.kind.replace(/^gemini-/, "Gemini · ").replace(/^3p-/, "Claude/GPT · ").replace(/weekly$/, "7d").replace(/five-hour$/, "5h") } : {}),
    usedPercent: window.usedPercent ?? 100,
    windowSeconds: (window.windowMinutes ?? 0) * 60,
    ...(window.resetsAt !== undefined ? { resetAtUnix: window.resetsAt } : {}),
  });
  const rejected = quota.status === "rejected";
  if (measured.length === 0 && !rejected) return null;
  const windows = measured.length > 0 ? measured : quota.windows;
  return {
    fetchedAtUnixMs: quota.observedAtUnixMs,
    ...(quota.provider === "gemini" ? { windows: windows.map(toWindow) } : {}),
    ...(quota.plan ? { planType: quota.plan } : {}),
    allowed: !rejected,
    limitReached: rejected,
    ...(windows[0] ? { primary: rejected && measured.length === 0 ? { ...toWindow(windows[0]), usedPercent: 100 } : toWindow(windows[0]) } : {}),
    ...(windows[1] ? { secondary: toWindow(windows[1]) } : {}),
  };
}

/**
 * The remembered model and effort for a session on `provider`. Claude with
 * nothing remembered starts on its current Opus rather than whatever the
 * account defaults to.
 */
function launchDefaults(defaults: SessionDefaults, provider: Provider): { model?: string; reasoningEffort?: string } {
  const choice = defaults[provider] ?? {};
  const model = choice.model ?? (provider === "claude" ? CLAUDE_DEFAULT_MODEL : undefined);
  return {
    ...(model ? { model } : {}),
    ...(choice.reasoningEffort ? { reasoningEffort: choice.reasoningEffort } : {}),
  };
}

/**
 * What a new session starts with beyond its provider: a model and effort that
 * override the remembered ones, a team, and a first message that goes out as
 * soon as the agent is ready.
 */
export type NewSessionLaunch = { resources?: WorkerSlot["resources"]; model?: string; reasoningEffort?: string; combo?: Combo; message?: string; attachments?: AttachmentSnapshot[]; /** The account to open the session on, instead of the one it was on or the active one. */ accountId?: string };

/** The adapter's alias for its current Opus. */
const CLAUDE_DEFAULT_MODEL = "opus";

/**
 * Saved defaults, read from either shape: per provider, or the earlier single
 * `{ provider, model, reasoningEffort }` that followed the last change.
 */
export function readSessionDefaults(raw: unknown): SessionDefaults {
  if (!raw || typeof raw !== "object") return {};
  const value = raw as Record<string, unknown>;
  const pick = (entry: unknown): ProviderChoice => {
    const choice = (entry ?? {}) as Record<string, unknown>;
    return {
      ...(typeof choice.model === "string" ? { model: choice.model } : {}),
      ...(typeof choice.reasoningEffort === "string" ? { reasoningEffort: choice.reasoningEffort } : {}),
    };
  };
  if (typeof value.provider === "string") {
    const provider = value.provider as Provider;
    return PROVIDERS.includes(provider) ? { [provider]: pick(value) } : {};
  }
  const defaults: SessionDefaults = {};
  for (const provider of PROVIDERS) if (value[provider]) defaults[provider] = pick(value[provider]);
  return defaults;
}

/** Resolves the current preset, so edits take effect and deleted teams fall back to one agent. */
export function projectTeamPreset(state: Pick<State, "projectTeamPresetIds" | "teamPresets" | "quickChats">, workspaceId: string): TeamPreset | undefined {
  if (state.quickChats.some((chat) => chat.workspaceId === workspaceId)) return undefined;
  return state.teamPresets.find((preset) => preset.id === state.projectTeamPresetIds[workspaceId]);
}

/** Where a new session starts when nobody asked for a provider: Claude when it can run, else the first that can. */
export function startingProvider(providers: readonly ProviderInfo[] | null): Provider {
  const ready = (provider: Provider) => {
    const info = providers?.find((entry) => entry.provider === provider);
    return !info || (!!info.resolved && info.auth?.loggedIn !== false);
  };
  return PROVIDERS.find(ready) ?? "claude";
}

/** Journal summary marking a planning turn as asked for; the engine writes it. */
export const PLAN_REQUESTED = "Asked the agent to read the plan document";

/** The engine's runtime for a goal, in the shape the screen reads. */
function runtimeFrom(runtime: BigThingRuntime): OdysseyRuntime {
  return {
    resumeAt: runtime.resumeAt ?? null,
    lastReason: runtime.lastReason,
    lastReasonAt: runtime.lastReasonAt,
    ticking: runtime.ticking,
    stalledSince: runtime.stalledSince ?? null,
    stallNotified: runtime.stallNotified,
    forecast: runtime.forecast ?? null,
  };
}

/** The live session a goal's record points at, by its id or its agent's. */
function sessionKeyForGoal(state: State, goalId: string, agentSessionId?: string | null): string | null {
  for (const [key, view] of Object.entries(state.odyssey)) if (view?.goal.id === goalId) return key;
  if (agentSessionId) {
    const live = Object.values(state.sessions).find((session) => session.snapshot.agentSessionId === agentSessionId);
    if (live) return live.handle.id;
  }
  return null;
}

type Get = () => State;
type Set = (partial: Partial<State>) => void;

function projectionFromSnapshot(snapshot: Snapshot): Projection {
  const projection = emptyProjection();
  projection.process = snapshot.process;
  projection.attachment = snapshot.attachment;
  projection.capabilities = snapshot.capabilities;
  projection.foreground = snapshot.foreground;
  projection.configOptions = snapshot.configOptions;
  projection.queue = snapshot.queue ?? EMPTY_QUEUE;
  for (const [id, patch] of Object.entries(snapshot.toolCalls)) projection.toolCalls.set(id, patch);
  for (const task of snapshot.backgroundTasks ?? []) projection.background.set(task.taskId, task);
  return projection;
}

/** Where a goal is being moved to: an open session, or a fresh one. */
type MoveTarget = { kind: "session"; sessionId: string } | { kind: "new"; agent: Provider };

/**
 * Attaches the renderer to a live actor: snapshot, retained history (replayed
 * in order, deduplicated by sequence), then the live stream. Events that
 * arrive while history is loading are buffered and applied after it.
 */
/** The reports a sent message carried: the host took them with it. */
function reportsSent(get: Get, set: Set, agentSessionId: string | null) {
  if (!agentSessionId || !get().nestedReports[agentSessionId]?.length) return;
  set({ nestedReports: { ...get().nestedReports, [agentSessionId]: [] } });
}

/**
 * Stops live sessions nobody has opened, and nothing has happened in, for
 * three days (see idleSessions.ts). The conversation stays in the list. A
 * session that is working, or on screen, is never stopped.
 */
async function stopIdleSessions(get: Get, set: Set): Promise<void> {
  const state = get();
  const candidates = Object.entries(state.sessions).map(([id, session]) => {
    const projection = session.projection;
    const busy =
      !!session.inFlightRequestId ||
      !!session.pendingPrompt ||
      projection.foreground === "running" ||
      projection.foreground === "awaiting_user" ||
      projection.autonomous.size > 0 ||
      [...projection.detached.values()].some((s) => s === "active" || s === "cancellation_requested") ||
      [...projection.inspector.agents.values()].some((agent) => agent.status === "working" || agent.status === "starting");
    return { id, sequence: projection.lastSequence, busy };
  });
  const onScreen = state.view.kind === "session" ? state.view.sessionId : null;
  const { touches, stop } = idleSweep(candidates, loadTouches(), Date.now(), onScreen);
  saveTouches(touches);
  for (const id of stop) {
    const session = get().sessions[id];
    if (!session) continue;
    try {
      await api.sessionStop(session.handle);
    } catch {
      continue;
    }
    const { [id]: _stopped, ...rest } = get().sessions;
    set({ sessions: rest, sessionOrder: get().sessionOrder.filter((s) => s !== id) });
    void get().loadRecords(session.workspaceId);
  }
}

async function attachLive(get: Get, set: Set, handle: SessionHandle, workspaceId: string, focus: boolean, localSetup?: LocalSessionSetup): Promise<void> {
  const snapshot = await api.sessionSnapshot(handle);
  const projection = projectionFromSnapshot(snapshot);
  const id = handle.id;
  const existing = get().sessions[id];
  const setup = localSetup ?? existing?.localSetup;
  const live: LiveSession = {
    ...(setup ? { localSetup: setup } : {}),
    handle,
    workspaceId,
    snapshot,
    projection,
    inFlightRequestId: null,
    steerInFlight: false,
    attention: existing?.attention ?? "none",
    openedAt: existing?.openedAt ?? Date.now(),
    turnStartedAt: null,
    usage: existing?.usage ?? EMPTY_SESSION_USAGE,
      lastEventAt: existing?.lastEventAt ?? null,
};
  const sameAttachment = existing?.workspaceId === workspaceId && existing.snapshot.provider === snapshot.provider
    && existing.snapshot.agentSessionId === snapshot.agentSessionId
    && (existing.snapshot.accountId ?? "default") === (snapshot.accountId ?? "default");
  const previousBrief = sameAttachment ? get().sideBrief[id] : undefined;
  const { [id]: _staleBrief, ...otherBriefs } = get().sideBrief;
  set({
    sessions: { ...get().sessions, [id]: live },
    sessionOrder: [...get().sessionOrder.filter((s) => s !== id), id],
    sideBrief: previousBrief ? { ...otherBriefs, [id]: previousBrief } : otherBriefs,
    ...(focus ? { view: { kind: "session", sessionId: id } as View, sessionTab: "transcript" as SessionTab } : {}),
  });
  // A continuation prepared before a restart still owes its first message
  // the brief.
  void restorePendingBrief(get, set, id);
  // Read any goal this session is driving now rather than when its tab is
  // first opened: a run has to continue whether or not anyone is looking.
  void get().loadOdyssey(id);
  void get().loadTeam(id);
  if (snapshot.agentSessionId) void get().loadNesting(workspaceId, snapshot.agentSessionId);

  let historyDone = false;
  const buffered: EventEnvelope[] = [];
  // Presentation batching (section 21.1): events are applied to the
  // projection as they arrive but the store publishes at most ~30 times per
  // second, so streaming tokens never rerender every pane per chunk.
  let queue: EventEnvelope[] = [];
  let flushTimer: ReturnType<typeof setTimeout> | null = null;
  const flush = () => {
    flushTimer = null;
    const events = queue;
    queue = [];
    const current = get().sessions[id];
    if (!current || events.length === 0) return;
    const view = get().view;
    const viewing = view.kind === "session" && view.sessionId === id;
    let attention = current.attention;
    let announcement: string | null = null;
    let turnStartedAt = current.turnStartedAt;
    const worker = isWorkerSession(get, id, current);
    for (const event of events) {
      if (BigInt(event.sequence) <= BigInt(current.projection.lastSequence)) continue;
      applyEvent(current.projection, event);
      // A worker reports to its orchestrator, whose Agents tab shows it; its
      // own turns finishing are not the user's to acknowledge.
      if (!viewing && !worker) attention = attentionFor(event, attention);
      const payload = event.payload;
      if (payload.type === "turn" && payload.effect === "started" && payload.kind === "foreground") {
        turnStartedAt = Date.now();
        void get().sampleUsage(id, "start");
      }
      if (payload.type === "quota") {
        const { type: _type, ...quota } = payload;
        if (sessionUsesActiveAccount(current.snapshot, get().accounts)) get().noteQuota(quota);
      }
      if (payload.type === "turn" && payload.effect === "settled" && payload.kind === "foreground") {
        turnStartedAt = null;
        void get().sampleUsage(id, "settle");
        announcement = turnAnnouncement(sessionTitle(current), payload.phase);
      } else if (payload.type === "permission_request") {
        announcement = t("{title}: the agent asked for input", { title: sessionTitle(current) });
        // A run's permission decisions are journalled by the engine.
      } else if (payload.type === "exited") {
        announcement = t("{title}: the agent exited", { title: sessionTitle(current) });
      }
    }
    if (viewing) attention = "none";
    set({ sessions: { ...get().sessions, [id]: { ...current, projection: { ...current.projection }, pendingPrompt: current.pendingPrompt && !promptPublished(current.projection, current.pendingPrompt.requestId) ? current.pendingPrompt : undefined, attention, turnStartedAt, lastEventAt: Date.now() } } });
    if (announcement) get().announce(announcement);
    if (!viewing && attention !== "none" && attention !== current.attention) {
      const settings = get().settings;
      const workspace = get().workspaces.find((w) => w.id === current.workspaceId);
      const name = workspace ? basenameOf(workspace.displayPath) : t("a workspace");
      if (settings && shouldNotify(settings, attention, current.workspaceId)) void notify(attention, name);
    }
  };
  const applyLive = (event: EventEnvelope) => {
    queue.push(event);
    if (flushTimer === null) flushTimer = setTimeout(flush, 33);
  };
  await api.sessionSubscribe(handle, (event: EventEnvelope) => {
    if (historyDone) applyLive(event);
    else buffered.push(event);
  });
  try {
    const history = await api.sessionHistory(handle, "0");
    const current = get().sessions[id];
    if (current) {
      const fresh = projectionFromSnapshot(current.snapshot);
      for (const event of history) applyEvent(fresh, event);
      set({ sessions: { ...get().sessions, [id]: { ...current, projection: fresh } } });
    }
  } finally {
    historyDone = true;
    for (const event of buffered) applyLive(event);
  }
}

/**
 * Whether a pending choice moves the conversation to a new session: another
 * provider, or a local chat becoming a local agent (or back), which run as
 * different programs. With `team`, the team the next message is for: a
 * Gemini session opened without one runs in a mode that cannot take one
 * while it runs, so helpers to lead move it to a new Gemini session too.
 */
export function switchesSession(session: { snapshot: Parameters<typeof isLocalChat>[0] & { leadsTeam?: boolean }; localSetup?: LocalSessionSetup } | undefined, pending: PendingProvider, team?: Combo): boolean {
  const snapshot = session?.snapshot;
  if (!snapshot || pending.provider !== snapshot.provider) return true;
  if (pending.provider === "gemini") return snapshot.leadsTeam === false && (team?.workers.length ?? 0) > 0;
  if (pending.provider !== "local" || !pending.model) return false;
  const harness = localAgentHarness(pending.model);
  if (harness && !isLocalChat(snapshot)) {
    const current = session?.localSetup;
    if (!current || current.model !== pending.model) return true;
    if ((current.resources?.referencedSkills ?? false) !== (pending.resources?.referencedSkills ?? false)) return true;
    return (["skills", "mcps", "excludedSkills"] as const).some((key) => {
      const active = new Set(current.resources?.[key]);
      const desired = new Set(pending.resources?.[key]);
      return active.size !== desired.size || [...active].some((id) => !desired.has(id));
    });
  }
  return parseLocalModel(pending.model).kind !== (isLocalChat(snapshot) ? "chat" : "agent");
}

const modelRequests = new Map<Provider, symbol>();

function refreshAccountModels(get: Get, set: Set, provider: Provider) {
  const { [provider]: _oldModels, ...providerModels } = get().providerModels;
  set({ providerModels, usage: { ...get().usage, [provider]: null } });
  void get().loadProviderModels(provider, true);
}

/** Receipt checks under way, by session and token, so a card redrawn while one runs does not ask again. */
const teamChatChecks = new Set<string>();

export const useStore = create<State>((set, get) => ({
  providers: null,
  providerVersions: null,
  providerUpdate: { running: null, results: {} },
  profiles: [],
  workspaces: [],
  quickChats: [],
  inspections: {},
  usage: {},
  quotas: {},
  records: {},
  repoInfo: {},
  odyssey: {},
  odysseyRuntime: {},
  odysseyAmendments: {},
  odysseyNotes: {},
  odysseyMovedAway: {},
  odysseyPlanChanges: {},
  odysseyQuestions: {},
  expandedArchives: {},
  view: { kind: "welcome" },
  sessions: {},
  sessionOrder: [],
  sideBrief: {},
  nestedReports: {},
  reporting: {},
  drafts: {},
  teams: {},
  jobs: {},
  teamChat: {},
  teamChatUnknown: {},
  ultraReviews: {},
  defaultCombo: EMPTY_COMBO,
  providerModels: {},
  pendingProvider: {},
  continuations: {},
  continuationHistory: {},
  teamPresets: [],
  helperPresets: [],
  defaultPresetId: null,
  projectTeamPresetIds: {},
  signIn: null,
  accounts: {},
  starting: null,
  settings: null,
  closeRequest: null,
  error: null,
  busy: null,
  focusComposerToken: 0,
  lockedResume: null,
  outbox: {},
  attachments: {},
  mentions: {},
  contextBundles: {},
  paletteOpen: false,
  sessionTab: "transcript",
  navHistory: [{ view: { kind: "welcome" }, tab: null }],
  navIndex: 0,
  odysseyFocus: null,
  changesFocus: null,
  transcriptFocus: null,
  transcriptSearchOpen: false,
  announcement: "",
  uiPrefs: DEFAULT_UI_PREFS,
  appUpdate: { current: null, available: null, checking: false, checkedAt: null, error: null, installing: null, prompt: false },
  bigThingDoc: {},
  // On the phone the sidebar is a drawer, closed until asked for.
  sidebarCollapsed: IS_REMOTE,
  sessionDefaults: {},
  observedImages: {},
  seenImageFiles: {},

  async bootstrap() {
    try {
      const [profiles, workspaces] = await Promise.all([api.executionProfiles(), api.workspaceList(), get().loadTeamPresets()]);
      set({ profiles, workspaces });
      void get().loadSettings();
      void get().loadUiPrefs();
      void get().loadContinuations();
      void get().loadProviders();
      // Whether a provider program has fallen behind: once shortly after
      // start, then twice a day. It asks npm, so it is never on the hot path.
      setTimeout(() => void get().loadProviderVersions(), 20_000);
      setInterval(() => void get().loadProviderVersions(), 12 * 60 * 60 * 1000);
      // ThingMaker's own releases: shortly after start (after the
      // preferences, which say whether to offer the popup), then every few
      // hours. The phone has no install of its own to update.
      if (!IS_REMOTE) {
        // Sessions left unopened for three days stop: a minute after start,
        // then every hour. Only the computer does this, never the phone.
        setTimeout(() => void stopIdleSessions(get, set), 60_000);
        setInterval(() => void stopIdleSessions(get, set), 60 * 60 * 1000);
        setTimeout(() => void get().checkForUpdate("startup"), 5_000);
        setInterval(() => void get().checkForUpdate("timer"), UPDATE_CHECK_EVERY_MS);
      }
      // Inspection never launches an agent; doing it up front lets the sidebar show
      // real trust state instead of "not inspected yet".
      for (const workspace of workspaces) {
        void get().inspectWorkspace(workspace.id);
        // Archive and pin flags live in desktop metadata; load them for every
        // workspace so archived sessions are hidden and the Pinned group is
        // complete on the first render, not after visiting each project.
        void get().loadRecords(workspace.id);
      }
      // The live sessions, attached again; the phone waits on them to return
      // to the conversation it had open.
      const rehydrated = get().rehydrate();
      void get().loadDefaultCombo();
      // Refresh sidebar usage when the Windows app is active. The host also
      // checks foreground ownership before starting a CLI, including for calls from the phone.
      const usageInterval = 5 * 60_000;
      const readUsage = () => {
        for (const provider of PROVIDERS) if (provider !== "local") void get().refreshUsage(provider);
      };
      if (!IS_REMOTE && navigator.userAgent.includes("Windows")) {
        let nextUsagePoll = 0;
        let usageTimer: ReturnType<typeof setTimeout>;
        const pollWhileFocused = () => {
          clearTimeout(usageTimer);
          const now = Date.now();
          if (now >= nextUsagePoll) {
            nextUsagePoll = now + usageInterval;
            readUsage();
          }
          usageTimer = setTimeout(pollWhileFocused, nextUsagePoll - now);
        };
        const appWindow = getCurrentWindow();
        let focusReported = false;
        void appWindow.onFocusChanged(({ payload: focused }) => {
          focusReported = true;
          if (focused) pollWhileFocused();
          else clearTimeout(usageTimer);
        }).then(async () => {
          if (await appWindow.isFocused() && !focusReported) pollWhileFocused();
        }).catch(() => undefined);
      } else {
        readUsage();
        setInterval(readUsage, usageInterval);
      }
      void get().loadQuickChats();
      void listen<JobView>(JOB_EVENT, (event) => get().noteJob(event.payload));
      void listen<{ handle: string }>(TEAM_CHAT_EVENT, (event) => void get().loadTeamChat(event.payload.handle));
      void listen<UltraReview>(ULTRA_REVIEW_EVENT, (event) => get().noteUltraReview(event.payload));
      void listen<BigThingEvent>(BIGTHING_EVENT, (event) => get().noteBigThing(event.payload));
      void listen<SessionTitleChange>(SESSION_TITLE_EVENT, (event) => get().noteSessionTitle(event.payload));
      void listen(CLOSE_REQUESTED_EVENT, async () => {
        let activity: { id: string; active: boolean; detachedCalls: number }[] = [];
        try {
          activity = await api.appActivity();
        } catch {
          activity = Object.values(get().sessions).map((s) => ({ id: s.handle.id, active: s.projection.foreground === "running", detachedCalls: 0 }));
        }
        set({ closeRequest: { activity } });
      });
      const place = IS_REMOTE ? rememberedPlace() : null;
      if (place && workspaces.some((w) => w.id === place.workspaceId)) {
        await returnTo(get, set, place, rehydrated);
      } else {
        // Someone part-way through the first-run guide comes back to it;
        // anyone who had projects before the guide existed never sees it.
        const setup = IS_REMOTE ? null : readSetup();
        if (!IS_REMOTE && !setup && workspaces.length > 0) writeSetup({ finished: true });
        // Never open on a temporary chat from the last run: it is being deleted.
        await get().loadQuickChats();
        const leaving = new Set(get().quickChats.filter((chat) => chat.deleting).map((chat) => chat.workspaceId));
        const first = workspaces.find((workspace) => !leaving.has(workspace.id));
        if (first && !(setup && !setup.finished)) {
          await get().selectWorkspace(first.id);
        }
      }
      placeRestored = true;
    } catch (error) {
      placeRestored = true;
      set({ error: asError(error) });
    }
  },

  async loadSettings() {
    try {
      set({ settings: await api.settingsGet() });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async saveSettings(settings) {
    try {
      set({ settings: await api.settingsSet(settings) });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async testNotification() {
    const sent = await notify("completed", t("a test workspace"));
    if (!sent) {
      set({ error: { code: "UNSUPPORTED", message: t("Notifications are not permitted by the operating system for ThingMaker."), retry: "user_action" } });
    }
  },

  dismissCloseRequest() {
    set({ closeRequest: null });
  },

  async hideToTray() {
    set({ closeRequest: null });
    try {
      await api.appHideToTray();
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async quitAndStop() {
    set({ closeRequest: null, busy: t("Stopping local tasks and quitting") });
    try {
      await api.appQuit(true);
    } catch (error) {
      set({ error: asError(error), busy: null });
    }
  },

  setView(view) {
    // On the phone, going somewhere closes the drawer.
    set({ view, ...(IS_REMOTE ? { sidebarCollapsed: true } : {}) });
    if (view.kind === "session") {
      const session = get().sessions[view.sessionId];
      // Opening a session restarts its idle clock (idleSessions.ts).
      if (session?.projection && !IS_REMOTE) saveTouches(touch(loadTouches(), view.sessionId, session.projection.lastSequence, Date.now()));
      if (session && session.attention !== "none") {
        set({ sessions: { ...get().sessions, [view.sessionId]: { ...session, attention: "none" } } });
      }
    }
  },

  navigate(direction) {
    const state = get();
    const index = navTarget(state.navHistory, state.navIndex, direction, (view) => viewExists(state, view));
    if (index === null) return;
    const { view, tab } = state.navHistory[index]!;
    // The index moves first, so the visit this causes is already the current
    // place and is not recorded as a new one.
    set({ navIndex: index, ...(tab ? { sessionTab: tab, transcriptSearchOpen: false } : {}) });
    if (view.kind === "workspace") void get().selectWorkspace(view.workspaceId);
    else get().setView(view);
    if (view.kind === "signin") void get().loadProviders();
    if (view.kind === "settings") void get().loadSettings();
  },

  async addWorkspaceByPicker(options) {
    try {
      const path = await api.workspacePick();
      return path ? await get().addWorkspaceByPath(path, options) : null;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    }
  },

  async addWorkspaceByPath(path, options) {
    if (!path.trim()) return null;
    set({ busy: t("Inspecting workspace"), error: null });
    try {
      const inspection = await api.workspaceInspect(path.trim());
      const workspaces = await api.workspaceList();
      set({
        workspaces,
        inspections: { ...get().inspections, [inspection.record.id]: inspection },
        ...(options?.stay ? {} : { view: { kind: "workspace", workspaceId: inspection.record.id } as View }),
      });
      return inspection.record.id;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    } finally {
      set({ busy: null });
    }
  },

  async selectWorkspace(workspaceId) {
    set({ view: { kind: "workspace", workspaceId }, ...(IS_REMOTE ? { sidebarCollapsed: true } : {}) });
    if (!get().inspections[workspaceId]) await get().inspectWorkspace(workspaceId);
  },

  async inspectWorkspace(workspaceId) {
    const record = get().workspaces.find((w) => w.id === workspaceId);
    if (!record) return null;
    try {
      const inspection = await api.workspaceInspect(record.canonicalRoot);
      set({ inspections: { ...get().inspections, [workspaceId]: inspection } });
      return inspection;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    }
  },

  async trust(workspaceId, state) {
    const inspection = get().inspections[workspaceId];
    if (!inspection) return;
    set({ busy: t("Recording trust decision"), error: null });
    try {
      const updated = await api.workspaceTrust(workspaceId, state, inspection.trustDigest);
      const workspaces = await api.workspaceList();
      set({ workspaces, inspections: { ...get().inspections, [workspaceId]: updated } });
      if (state === "trusted_local") void get().loadRecords(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
      // The digest may be stale: re-inspect so the user sees the new sources.
      await get().inspectWorkspace(workspaceId);
    } finally {
      set({ busy: null });
    }
  },

  async loadProviders() {
    try {
      set({ providers: await api.providersStatus() });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  noteSessionTitle({ agentSessionId, title }) {
    const records = get().records;
    for (const [workspaceId, list] of Object.entries(records)) {
      if (!list.some((record) => record.agentSessionId === agentSessionId)) continue;
      set({ records: { ...records, [workspaceId]: list.map((record) => (record.agentSessionId === agentSessionId ? { ...record, autoTitle: title } : record)) } });
      return;
    }
  },

  async loadProviderVersions(askLatest = true) {
    try {
      const versions = await api.providerVersions(askLatest);
      set({ providerVersions: Object.fromEntries(versions.map((version) => [version.provider, version])) });
    } catch {
      // A version check is advice; failing it must not raise an error banner.
    }
  },

  async updateProvider(provider) {
    if (get().providerUpdate.running) return;
    const before = get().providerVersions?.[provider]?.installed;
    set({ providerUpdate: { ...get().providerUpdate, running: provider } });
    let result: { ok: boolean; text: string };
    try {
      const { version } = await api.providerUpdate(provider);
      set({ providerVersions: { ...get().providerVersions, [provider]: version } });
      const now = version.installed ?? t("the new version");
      result = {
        ok: true,
        text:
          now === before
            ? t("Already on {version}.", { version: now })
            : before
              ? t("Updated from {before} to {version}. Sessions already open keep the version they started with; new sessions use {version}.", { before, version: now })
              : t("Updated to {version}. Sessions already open keep the version they started with; new sessions use {version}.", { version: now }),
      };
    } catch (error) {
      result = { ok: false, text: asError(error).message };
    }
    set({ providerUpdate: { running: null, results: { ...get().providerUpdate.results, [provider]: result } } });
  },

  async refreshUsage(provider) {
    // Ask each CLI for its own account usage without spending a model turn.
    try {
      const quota = await api.providerQuota(provider);
      // A reading is for the account it was asked about, or it is not one.
      if (quota) {
        if (quota.provider !== provider) throw new Error("reading for another provider");
        get().noteQuota(quota);
      }
    } catch {
      // A reading that cannot be taken leaves the last one in place; it is
      // evidence, never a decision. Claude's refusal-based reading is the
      // fallback when Claude Code cannot be asked.
      if (provider === "claude") return get().refreshClaudeUsage();
    }
    return get().usage[provider] ?? null;
  },

  noteQuota(quota) {
    const quotas = { ...get().quotas, [quota.provider]: quota };
    const snapshot = usageFromQuota(quota);
    const usage = { ...get().usage };
    // A report with no percentages (what a Claude session says on every turn)
    // says only "allowed": it clears a refusal recorded earlier — a turn that
    // got an answer is the positive evidence the refusal was waiting for —
    // but leaves a measured reading in place, so the sidebar keeps it.
    if (snapshot) usage[quota.provider] = snapshot;
    else if (quota.status !== "rejected" && usage[quota.provider]?.limitReached) usage[quota.provider] = null;
    set({ quotas, usage });
  },

  /**
   * Re-reads the account behind one session after a failure that looked like
   * a quota wall.
   *
   * The two accounts report in different ways and the difference matters: the
   * OpenAI one has an endpoint, so it is asked; the Claude one does not, so
   * the failure message *is* the reading, and asking anything else would be
   * asking a question nothing answers.
   */
  async resampleAccount(sessionId, message) {
    const provider = get().sessionAgent(sessionId);
    if (provider !== "claude") {
      await get().refreshUsage(provider);
      return;
    }
    // The message is the reading when it carries one; the probe is the
    // fallback for a refusal that named nothing useful.
    if (!get().noteClaudeLimit(message)) await get().refreshClaudeUsage();
  },

  sessionAgent(sessionId) {
    return get().sessions[sessionId]?.snapshot.provider ?? "claude";
  },

  usageFor(provider) {
    return get().usage[provider] ?? null;
  },

  /**
   * Reads a failed turn's error as the Claude account being spent, and keeps
   * it. Returns whether it was one, so the caller can tell a quota wall from
   * an ordinary failure without parsing the message twice.
   */
  noteClaudeLimit(message) {
    const snapshot = claudeUsageFrom(message, Date.now());
    if (!snapshot) return false;
    set({ usage: { ...get().usage, claude: snapshot } });
    return true;
  },

  /**
   * Re-reads the Claude account's state — which is a clock, not a question
   * anything answers (docs/plans/odyssey-second-orchestrator.md §2.4).
   *
   * This started out asking the adapter for its model catalog, on the theory
   * that a spent account would refuse it. It does not: `session/new` succeeds
   * and the catalog comes back in full while every prompt is refused
   * (verified against the live adapter). So the probe proves *sign-in*, never
   * headroom — and using it here cleared the limit a minute after it was
   * recorded, wiped the countdown off the screen and sent the runner back
   * into a refusal. Park, clear, resume, refuse, park, once a minute.
   *
   * The only two honest facts on this side are the refusal and the reset time
   * printed in it, so that is all this uses. Inside the window the recorded
   * limit stands and the goal counts down to it; once the window has passed
   * the limit is dropped so the run may try again, and if it is refused again
   * the refusal re-parks it with the new time. That costs one prompt per
   * window, which is the cheapest possible way to learn something nothing
   * will tell you.
   */
  async refreshClaudeUsage() {
    const held = get().usage.claude;
    // Only a refusal's reading is a clock to wait out; a measured one stands.
    if (!held?.limitReached) return held ?? null;
    const resetAt = held.primary?.resetAtUnix;
    const windowOver = resetAt !== undefined && Date.now() >= resetAt * 1000;
    // Retry well before the reset the refusal named, because that time is
    // not to be trusted as a floor. These windows roll — §6's lesson on the
    // other account — and a refusal can also name a window that was never
    // this run's to begin with: one was recorded here against the wrong
    // account entirely and would have parked a working run for three hours.
    //
    // A refused prompt costs nothing, so the cheap thing is to keep asking.
    const worthRetrying = Date.now() - held.fetchedAtUnixMs >= CLAUDE_RETRY_MS;
    if (!windowOver && !worthRetrying) return held;
    set({ usage: { ...get().usage, claude: null } });
    return null;
  },

  async sampleUsage(sessionId, phase) {
    const before = get().sessions[sessionId];
    if (!before) return;
    // Per-turn window deltas need percentages, which only Codex reports. The
    // readings arrive as quota events on the session's own stream, so a turn
    // boundary reads the latest one rather than asking anything.
    if (get().sessionAgent(sessionId) !== "codex") return;
    const snapshot = before.projection.quota ? usageFromQuota(before.projection.quota)
      : sessionUsesActiveAccount(before.snapshot, get().accounts) ? get().usage.codex ?? null : null;
    const patch = (usage: Partial<SessionUsage>) => {
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, usage: { ...current.usage, ...usage } } } });
    };
    const current = get().sessions[sessionId];
    if (!current || !snapshot) return;
    const usage = current.usage;
    if (phase === "start") {
      patch({ sampling: false, error: null, turnStart: snapshot, latest: snapshot });
      return;
    }
    if (phase === "tick" || !usage.turnStart) {
      patch({ sampling: false, error: null, latest: snapshot });
      return;
    }
    const primary = windowDelta(usage.turnStart.primary, snapshot.primary);
    const secondary = windowDelta(usage.turnStart.secondary, snapshot.secondary);
    patch({
      sampling: false,
      error: null,
      latest: snapshot,
      turnStart: null,
      lastTurn: { primary, secondary, settledAt: Date.now() },
      totalPrimary: usage.totalPrimary + (primary?.delta ?? 0),
      totalSecondary: usage.totalSecondary + (secondary?.delta ?? 0),
      turns: usage.turns + 1,
      resets: usage.resets + (primary?.reset ? 1 : 0) + (secondary?.reset ? 1 : 0),
    });
  },

  async loadRecords(workspaceId) {
    try {
      const records = await api.sessionRecords(workspaceId);
      set({ records: { ...get().records, [workspaceId]: records } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async archiveSession(workspaceId, agentSessionId, archived) {
    try {
      await api.sessionArchive(workspaceId, agentSessionId, archived);
      await get().loadRecords(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async stopAndArchive(workspaceId, agentSessionId, liveId) {
    if (liveId && get().sessions[liveId]) {
      await get().stop(liveId);
      // Kept running when asked, or it failed to stop: it stays listed.
      if (get().sessions[liveId]) return;
    }
    await get().archiveSession(workspaceId, agentSessionId, true);
  },

  /**
   * Reads the workspace's git head for the top bar. A folder that is not a
   * repository is a normal case, not an error: it records `null` and stays
   * quiet rather than raising the global error banner.
   */
  async loadRepoInfo(workspaceId) {
    try {
      const info = await api.gitInfo(workspaceId);
      set({ repoInfo: { ...get().repoInfo, [workspaceId]: info } });
    } catch {
      set({ repoInfo: { ...get().repoInfo, [workspaceId]: null } });
    }
  },

  /** Blank clears the overlay, restoring the title derived from the transcript. */
  async renameSession(workspaceId, agentSessionId, title) {
    try {
      await api.sessionRename(workspaceId, agentSessionId, title && title.trim() ? title.trim() : null);
      await get().loadRecords(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Reads the goal for a session. A session with no goal records `null`, which
   * is how the screen tells "none yet" from "not read yet".
   */
  async loadOdyssey(sessionId) {
    const session = get().sessions[sessionId];
    const agentSessionId = session?.snapshot.agentSessionId;
    if (!session || !agentSessionId) return;
    try {
      // The goal is keyed by the desktop session record, which the supervisor
      // resolves from the agent's id.
      const view = await api.odysseyForSession(session.workspaceId, agentSessionId);
      set({ odyssey: { ...get().odyssey, [sessionId]: view } });
      // Queued amendments survive a reload; without this they would be in the
      // record and invisible, and the next prompt would not carry them.
      if (view) {
        await get().odysseyLoadAmendments(sessionId, view.goal.id);
        await get().odysseyLoadInbox(sessionId, view.goal.id);
        // What the engine is doing now; later changes arrive as events.
        const runtime = await api.bigthingRuntime(view.goal.id).catch(() => null);
        if (runtime && typeof runtime === "object" && "lastReason" in runtime) set({ odysseyRuntime: { ...get().odysseyRuntime, [sessionId]: runtimeFrom(runtime) } });
      }
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async createOdyssey(sessionId, request) {
    try {
      const view = await api.odysseyCreate(request);
      set({ odyssey: { ...get().odyssey, [sessionId]: view } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async editOdysseyGoal(sessionId, id, edit) {
    try {
      await api.odysseyEditGoal(id, edit);
      await get().refreshOdyssey(sessionId, id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async deleteOdyssey(sessionId, id) {
    try {
      await api.odysseyDelete(id);
      set({ odyssey: { ...get().odyssey, [sessionId]: null } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async addMilestone(sessionId, request) {
    try {
      await api.odysseyAddMilestone(request);
      await get().refreshOdyssey(sessionId, request.odysseyId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async editMilestone(sessionId, id, edit) {
    const current = get().odyssey[sessionId];
    try {
      await api.odysseyEditMilestone(id, edit);
      if (current) await get().refreshOdyssey(sessionId, current.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async reorderMilestones(sessionId, odysseyId, orderedIds) {
    try {
      await api.odysseyReorderMilestones(odysseyId, orderedIds);
      await get().refreshOdyssey(sessionId, odysseyId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async deleteMilestone(sessionId, id) {
    const current = get().odyssey[sessionId];
    try {
      await api.odysseyDeleteMilestone(id);
      if (current) await get().refreshOdyssey(sessionId, current.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async addStep(sessionId, milestoneId, title, detail = "", dependsOn = []) {
    const current = get().odyssey[sessionId];
    try {
      await api.odysseyAddStep(milestoneId, title, detail, dependsOn);
      if (current) await get().refreshOdyssey(sessionId, current.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setStepState(sessionId, id, state) {
    const current = get().odyssey[sessionId];
    try {
      await api.odysseySetStepState(id, state);
      if (current) await get().refreshOdyssey(sessionId, current.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyRequestTasks(sessionId, milestoneIndex) {
    const view = get().odyssey[sessionId];
    const milestone = view?.milestones[milestoneIndex];
    if (!view || !milestone) return;
    await get().odysseyAddAmendment(sessionId, {
      odysseyId: view.goal.id,
      note: `Break milestone ${milestoneIndex + 1} ("${milestone.title}") into tasks, each one thing a subagent or worker can be given on its own and sized in AI-agent time — about 10–30 minutes of agent work each, not human-effort estimates — cut so that independent tasks can run in parallel; put depends: under a task only when it truly cannot start until an earlier one has finished. Send them in a BIGTHING-AMEND block as revise: ${milestoneIndex + 1} with step: lines. Change nothing else about the milestone.`,
      refs: [],
    });
  },

  async deleteStep(sessionId, id) {
    const current = get().odyssey[sessionId];
    try {
      await api.odysseyDeleteStep(id);
      if (current) await get().refreshOdyssey(sessionId, current.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Starts or resumes a run. The engine in the host runs it (ADR-010); this
   * hands it the user's "go" and shows what the record says.
   */
  async odysseyStart(sessionId) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingStart(view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Points this goal at another session. The record moves, the transcript
   * does not: the new session is briefed from the record and the run's notes.
   * The current turn is cancelled first, so there is never more than one
   * orchestrator on the tree.
   */
  async odysseyMoveTo(sessionId, target) {
    const view = get().odyssey[sessionId];
    const session = get().sessions[sessionId];
    if (!view || !session) return;
    const running = session.projection.foreground === "running" || session.projection.foreground === "awaiting_user";
    if (target.kind === "session" && !get().sessions[target.sessionId]) {
      set({ error: asError(new Error(t("that session is not attached any more"))) });
      return;
    }
    try {
      const confirmed = await api.confirmDialog({
        title: t("Move this run to another session?"),
        message: [
          running ? t("The turn running now is cancelled first.") : "",
          t("The goal, its milestones and its history move across. The conversation does not: the new session is briefed from the record and from `{note}`, and starts with no memory of this one.", { note: ODYSSEY_STATE_NOTE }),
          target.kind === "new" ? t("A fresh {provider} session is opened for it.", { provider: PROVIDER_LABELS[target.agent] }) : "",
        ]
          .filter(Boolean)
          .join(" "),
        okLabel: t("Move the run"),
        cancelLabel: t("Leave it here"),
        warning: running,
      });
      if (!confirmed) return;
    } catch (error) {
      set({ error: asError(error) });
      return;
    }
    set({ busy: t("Moving the run") });
    try {
      await api.bigthingMove(view.goal.id, target.kind === "session" ? { kind: "session", handle: target.sessionId } : { kind: "new", provider: target.agent });
      get().announce(t("{title}: moved to another session", { title: view.goal.title }));
    } catch (error) {
      set({ error: asError(error) });
    } finally {
      set({ busy: null });
    }
  },

  /** Pauses before the next continuation; an in-flight turn is left alone. */
  async odysseyPause(sessionId, reason) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingPause(view.goal.id, reason);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /** Asks the engine to look at the goal now rather than on its next tick. */
  async odysseyTick(sessionId) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    await api.bigthingTick(view.goal.id).catch(() => undefined);
  },

  /** What the engine tells the interface (ADR-010). */
  noteBigThing(event) {
    const state = get();
    switch (event.kind) {
      case "changed": {
        const key = sessionKeyForGoal(state, event.goalId, event.agentSessionId);
        if (!key) return;
        void get().refreshOdyssey(key, event.goalId);
        void get().odysseyLoadInbox(key, event.goalId);
        void get().odysseyLoadAmendments(key, event.goalId);
        void get().odysseyRefreshNotes(key);
        return;
      }
      case "runtime": {
        const handle = event.runtime.sessionHandle;
        const key = handle && state.sessions[handle] ? handle : sessionKeyForGoal(state, event.goalId);
        if (key) set({ odysseyRuntime: { ...get().odysseyRuntime, [key]: runtimeFrom(event.runtime) } });
        return;
      }
      case "announce":
        get().announce(event.text);
        return;
      case "notify": {
        const settings = state.settings;
        const kind: AttentionKind = event.attention === "needs_input" ? "needs_input" : event.attention === "blocked" ? "failed" : "completed";
        const workspace = state.workspaces.find((entry) => entry.id === event.workspaceId);
        if (settings && shouldNotify(settings, kind, event.workspaceId)) void notify(kind, workspace ? basenameOf(workspace.displayPath) : t("a workspace"));
        return;
      }
      case "session_named":
        void get().loadRecords(event.workspaceId);
        return;
      case "nested_report": {
        // One report per fork waits: its latest.
        const held = (get().nestedReports[event.parentAgentSessionId] ?? []).filter((report) => report.forkAgentSessionId !== event.report.forkAgentSessionId);
        set({ nestedReports: { ...get().nestedReports, [event.parentAgentSessionId]: [...held, event.report] } });
        return;
      }
      case "document_loaded":
        // Waits in the Big Thing tab, which reads it when opened; the agent
        // tells the user to go there, so the tab is not switched under them.
        if (state.sessions[event.handle]) set({ bigThingDoc: { ...get().bigThingDoc, [event.handle]: event.path } });
        return;
      case "session_opened": {
        void get().loadRecords(event.workspaceId);
        if (state.sessions[event.handle]) return;
        void api
          .sessionListOpen()
          .then((open) => open.find((entry) => entry.handle.id === event.handle))
          .then((entry) => (entry && !get().sessions[event.handle] ? attachLive(get, set, entry.handle, event.workspaceId, false, entry.localSetup) : undefined))
          .catch(() => undefined);
        return;
      }
      case "moved": {
        const from = event.fromAgentSessionId ? Object.values(state.sessions).find((session) => session.snapshot.agentSessionId === event.fromAgentSessionId)?.handle.id : sessionKeyForGoal(state, event.goalId);
        const view = from ? state.odyssey[from] : null;
        if (from && from !== event.toHandle) {
          set({
            odyssey: { ...get().odyssey, [from]: null },
            odysseyMovedAway: { ...get().odysseyMovedAway, [from]: { goalId: event.goalId, title: view?.goal.title ?? t("The run"), to: event.toHandle } },
          });
        }
        if (get().sessions[event.toHandle]) void get().loadOdyssey(event.toHandle);
        return;
      }
    }
  },

  /**
   * Queues something for the model to fold into a goal that is already
   * running. The engine carries it on the next prompt; the model decides
   * where it belongs.
   */
  async odysseyAddAmendment(sessionId, request) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingAmend(request);
      await get().odysseyLoadAmendments(sessionId, view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyDecidePlanChange(sessionId, id, decision, note) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingDecidePlanChange(view.goal.id, id, decision === "apply", note);
      await get().odysseyLoadInbox(sessionId, view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyDecideApproval(sessionId, id, decision, comment) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingDecideApproval(view.goal.id, id, decision, comment);
      await get().odysseyLoadInbox(sessionId, view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyEditStep(sessionId, stepId, edit) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.odysseyEditStep(stepId, edit);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyAnswerQuestion(sessionId, id, answer) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingAnswer(view.goal.id, id, answer);
      await get().odysseyLoadInbox(sessionId, view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyDiscardAmendment(sessionId, id) {
    const view = get().odyssey[sessionId];
    try {
      await api.odysseyAmendSetState(id, "discarded");
      if (view) await get().odysseyLoadAmendments(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async odysseyLoadInbox(sessionId, odysseyId) {
    try {
      const [planChanges, questions] = await Promise.all([api.odysseyPlanChangeList(odysseyId), api.odysseyQuestionList(odysseyId)]);
      set({ odysseyPlanChanges: { ...get().odysseyPlanChanges, [sessionId]: planChanges }, odysseyQuestions: { ...get().odysseyQuestions, [sessionId]: questions } });
    } catch {
      // A missing table on an older build is not an error worth a banner.
    }
  },

  async odysseyLoadAmendments(sessionId, odysseyId) {
    try {
      const list = await api.odysseyAmendList(odysseyId);
      set({ odysseyAmendments: { ...get().odysseyAmendments, [sessionId]: list } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Asks the session's model to turn the goal's plan document into
   * milestones. The goal stays a draft until the user starts it.
   */
  async odysseyRequestPlan(sessionId) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingRequestPlan(view.goal.id);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /** Runs a milestone's check here; the engine records the verdict. */
  async odysseyRunCheck(sessionId, milestoneId) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingRunCheck(view.goal.id, milestoneId);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /** The user ticking a manual milestone: the user is the evidence. */
  async odysseyVerifyManually(sessionId, milestoneId) {
    const view = get().odyssey[sessionId];
    if (!view) return;
    try {
      await api.bigthingVerify(view.goal.id, milestoneId);
      await get().refreshOdyssey(sessionId, view.goal.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Closes out work the runtime never reported finishing.
   *
   * A subagent or child call leaves the inspector only when the runtime says
   * it ended. A process that is killed or dies with its provider never sends
   * that, so the node stays `working` and the activity strip reports it for
   * ever. The projection is a presentation cache (ADR-06), not history, so
   * correcting it here loses nothing: the agent's transcript is untouched, and a
   * `refreshSnapshot` would rebuild whatever the runtime still believes.
   */
  clearStaleActivity(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    const { inspector, detached } = session.projection;
    const now = Date.now();
    for (const [id, agent] of inspector.agents) {
      if (agent.status === "working" || agent.status === "starting") {
        inspector.agents.set(id, { ...agent, status: "idle", outcome: null, generationFinishedAtUnixMs: now });
      }
    }
    for (const [call, state] of detached) {
      if (state === "active" || state === "cancellation_requested") detached.set(call, "completed");
    }
    set({ sessions: { ...get().sessions, [sessionId]: { ...session, projection: { ...session.projection } } } });
  },

  async odysseyRefreshNotes(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    const notes = await api.odysseyWorkspaceNotes(session.workspaceId).catch(() => null);
    set({ odysseyNotes: { ...get().odysseyNotes, [sessionId]: notes } });
  },

  /** Re-reads one goal by id after a write, so the screen shows the record. */
  async refreshOdyssey(sessionId, id) {
    const view = await api.odysseyView(id);
    set({ odyssey: { ...get().odyssey, [sessionId]: view } });
  },

  async pinSession(workspaceId, agentSessionId, pinned) {
    try {
      await api.sessionPin(workspaceId, agentSessionId, pinned);
      await get().loadRecords(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  toggleArchiveList(workspaceId) {
    set((state) => ({
      expandedArchives: { ...state.expandedArchives, [workspaceId]: !state.expandedArchives[workspaceId] },
    }));
  },

  /**
   * Rebuilds live sessions from the supervisor after a renderer reload
   * (REC-04): snapshot + retained history, then the live stream. Nothing is
   * re-sent; the supervisor's event log is the only source.
   */
  async rehydrate() {
    const observed = Object.values(get().sessions).map((session) => session.handle);
    let open: Awaited<ReturnType<typeof api.sessionListOpen>> = [];
    try {
      open = await api.sessionListOpen();
    } catch {
      return;
    }
    // A session stopped on the other screen is no longer live. Remove its
    // stale attention marker too, but keep durable records and drafts.
    // Do not remove attachments opened/replaced while this request was away.
    const sessions = { ...get().sessions };
    let view = get().view;
    let removed = false;
    for (const handle of observed) {
      const session = sessions[handle.id];
      if (!session || session.handle.attachmentGeneration !== handle.attachmentGeneration) continue;
      if (open.some((entry) => entry.handle.id === handle.id && entry.handle.attachmentGeneration === handle.attachmentGeneration)) continue;
      delete sessions[handle.id];
      removed = true;
      if (view.kind === "session" && view.sessionId === handle.id) view = { kind: "workspace", workspaceId: session.workspaceId };
    }
    if (removed) set({ sessions, sessionOrder: get().sessionOrder.filter((id) => !!sessions[id]), view });
    for (const entry of open) {
      if (get().sessions[entry.handle.id]) continue;
      const workspaceId = entry.workspaceId ?? get().workspaces.find((w) => w.canonicalRoot === entry.root)?.id;
      if (!workspaceId) continue;
      try {
        await attachLive(get, set, entry.handle, workspaceId, false, entry.localSetup);
      } catch (error) {
        set({ error: asError(error) });
      }
    }
  },

  async loadOutbox(workspaceId) {
    try {
      const entries = await api.outboxList(workspaceId);
      set({ outbox: { ...get().outbox, [workspaceId]: entries } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async discardOutbox(workspaceId, requestId) {
    try {
      await api.outboxResolve(requestId, "discarded by user");
      await get().loadOutbox(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  /**
   * Explicit resend of an uncertain submission: a new request id is used and
   * the old entry is closed with a pointer to it (V08: never automatic).
   */
  async resendOutbox(workspaceId, entry) {
    const target = Object.values(get().sessions).find((s) => s.workspaceId === workspaceId && s.snapshot.agentSessionId === entry.agentSessionId);
    if (!target || entry.text === null) {
      set({ error: { code: "NOT_READY", message: t("Resume the session this prompt belonged to before resending it."), retry: "user_action" } });
      return;
    }
    const requestId = crypto.randomUUID();
    try {
      const response = await api.sessionSubmit(target.handle, requestId, entry.text);
      if (response.outcome.outcome === "accepted") {
        await api.outboxResolve(entry.record.requestId, `resent as ${requestId}`);
      } else {
        set({ error: response.outcome.error });
      }
      await get().loadOutbox(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  dismissLockedResume() {
    set({ lockedResume: null });
  },

  setError(error) {
    set({ error: asError(error) });
  },

  addAttachments(sessionId, snapshots) {
    const current = get().attachments[sessionId] ?? [];
    const merged = [...current];
    for (const snapshot of snapshots) if (!merged.some((a) => a.id === snapshot.id)) merged.push(snapshot);
    if (merged.length > 8) {
      set({ error: { code: "LIMIT_EXCEEDED", message: t("At most eight attachments per prompt."), retry: "user_action" } });
      return;
    }
    set({ attachments: { ...get().attachments, [sessionId]: merged } });
  },

  removeAttachment(sessionId, id) {
    set({ attachments: { ...get().attachments, [sessionId]: (get().attachments[sessionId] ?? []).filter((a) => a.id !== id) } });
  },

  addMention(sessionId, mention) {
    const current = get().mentions[sessionId] ?? [];
    if (current.some((m) => m.relativePath === mention.relativePath && JSON.stringify(m.mode) === JSON.stringify(mention.mode))) return;
    set({ mentions: { ...get().mentions, [sessionId]: [...current, mention] } });
  },

  updateMention(sessionId, index, mention) {
    const current = [...(get().mentions[sessionId] ?? [])];
    if (!current[index]) return;
    current[index] = mention;
    set({ mentions: { ...get().mentions, [sessionId]: current } });
  },

  removeMention(sessionId, index) {
    set({ mentions: { ...get().mentions, [sessionId]: (get().mentions[sessionId] ?? []).filter((_, i) => i !== index) } });
  },

  async loadContextBundles(workspaceId) {
    try {
      const bundles = (await api.prefGet<ContextBundle[]>(`context_bundles:${workspaceId}`)) ?? [];
      set({ contextBundles: { ...get().contextBundles, [workspaceId]: bundles } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async saveContextBundle(workspaceId, bundle) {
    const next = [...(get().contextBundles[workspaceId] ?? []).filter((b) => b.id !== bundle.id), bundle];
    try {
      await api.prefSet(`context_bundles:${workspaceId}`, next);
      set({ contextBundles: { ...get().contextBundles, [workspaceId]: next } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async deleteContextBundle(workspaceId, id) {
    const next = (get().contextBundles[workspaceId] ?? []).filter((b) => b.id !== id);
    try {
      await api.prefSet(`context_bundles:${workspaceId}`, next);
      set({ contextBundles: { ...get().contextBundles, [workspaceId]: next } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  applyContextBundle(sessionId, bundle) {
    for (const ref of bundle.refs) get().addMention(sessionId, ref);
    if (bundle.instructions.trim()) {
      const draft = get().drafts[sessionId] ?? "";
      set({ drafts: { ...get().drafts, [sessionId]: draft ? `${draft}\n${bundle.instructions}` : bundle.instructions } });
    }
    set({ sessionTab: "transcript" });
    get().focusComposer();
  },

  setPaletteOpen(open) {
    set({ paletteOpen: open });
  },

  setSessionTab(tab) {
    set({ sessionTab: tab });
  },

  openBigThingAt(sessionId, milestone) {
    set({ sessionTab: "odyssey", odysseyFocus: milestone === null ? null : { sessionId, milestone } });
  },

  showInTranscript(sessionId, cardKey) {
    set({ sessionTab: "transcript", transcriptSearchOpen: false, transcriptFocus: { sessionId, cardKey } });
  },

  clearJumpFocus() {
    set({ odysseyFocus: null, transcriptFocus: null });
  },

  sendDocToBigThing(sessionId, path) {
    set({ bigThingDoc: { ...get().bigThingDoc, [sessionId]: path }, sessionTab: "odyssey" });
  },

  takeBigThingDoc(sessionId) {
    const path = get().bigThingDoc[sessionId] ?? null;
    if (path) {
      const { [sessionId]: _taken, ...rest } = get().bigThingDoc;
      set({ bigThingDoc: rest });
    }
    return path;
  },

  setTranscriptSearchOpen(open) {
    set({ transcriptSearchOpen: open, sessionTab: open ? "transcript" : get().sessionTab });
  },

  toggleSidebar() {
    set({ sidebarCollapsed: !get().sidebarCollapsed });
  },

  announce(text) {
    // Re-announce identical text by toggling a trailing space.
    set({ announcement: get().announcement === text ? `${text} ` : text });
  },

  async loadUiPrefs() {
    try {
      const stored = await api.prefGet<Partial<UiPrefs>>("ui_prefs");
      if (stored) set({ uiPrefs: { ...DEFAULT_UI_PREFS, ...stored } });
      set({ sessionDefaults: readSessionDefaults(await api.prefGet<unknown>("session_defaults")) });
    } catch {
      // Preferences are optional conveniences.
    }
  },

  async wipeSessions(workspaceId) {
    try {
      const report = await api.workspaceWipeSessions(workspaceId);
      await get().loadRecords(workspaceId);
      return report;
    } catch (error) {
      get().setError(error);
      return null;
    }
  },

  async renameWorkspaceFolder(workspaceId, newName) {
    try {
      const report = await api.workspaceRenameFolder(workspaceId, newName);
      set({ workspaces: await api.workspaceList() });
      await get().inspectWorkspace(workspaceId);
      void get().loadRecords(workspaceId);
      return report;
    } catch (error) {
      get().setError(error);
      return null;
    }
  },

  async loadQuickChats() {
    try {
      const list = await api.quickChatList();
      set({ quickChats: Array.isArray(list) ? list : [] });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async newQuickChat() {
    try {
      const { chat, workspace } = await api.quickChatCreate(false);
      set({ quickChats: [...get().quickChats.filter((entry) => entry.workspaceId !== chat.workspaceId), chat], workspaces: await api.workspaceList() });
      await get().selectWorkspace(workspace.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setQuickChatTemporary(workspaceId, temporary) {
    try {
      const chat = await api.quickChatSetTemporary(workspaceId, temporary);
      set({ quickChats: get().quickChats.map((entry) => (entry.workspaceId === workspaceId ? chat : entry)) });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async deleteQuickChat(workspaceId, confirmed = false) {
    if (!confirmed) {
      const ok = await api
        .confirmDialog({
          title: t("Delete this chat?"),
          message: t("The conversation and its folder are deleted from this computer. This can't be undone."),
          okLabel: t("Delete"),
          cancelLabel: t("Cancel"),
          warning: true,
        })
        .catch(() => false);
      if (!ok) return false;
    }
    try {
      await api.quickChatDelete(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
      return false;
    }
    const view = get().view;
    const sessions = Object.fromEntries(Object.entries(get().sessions).filter(([, session]) => session.workspaceId !== workspaceId));
    const leaving = (view.kind === "workspace" && view.workspaceId === workspaceId) || (view.kind === "session" && !sessions[view.sessionId]);
    const strip = <T>(record: Record<string, T>) => {
      const next = { ...record };
      delete next[workspaceId];
      return next;
    };
    set({
      quickChats: get().quickChats.filter((entry) => entry.workspaceId !== workspaceId),
      workspaces: get().workspaces.filter((w) => w.id !== workspaceId),
      sessions,
      sessionOrder: get().sessionOrder.filter((id) => !!sessions[id]),
      inspections: strip(get().inspections),
      records: strip(get().records),
      outbox: strip(get().outbox),
      contextBundles: strip(get().contextBundles),
      ...(leaving ? { view: { kind: "welcome" } as View } : {}),
    });
    return true;
  },

  async removeWorkspace(workspaceId) {
    try {
      await api.workspaceRemove(workspaceId);
      const view = get().view;
      const leaving = (view.kind === "workspace" && view.workspaceId === workspaceId) || (view.kind === "session" && get().sessions[view.sessionId]?.workspaceId === workspaceId);
      const strip = <T>(record: Record<string, T>) => {
        const next = { ...record };
        delete next[workspaceId];
        return next;
      };
      set({
        workspaces: get().workspaces.filter((w) => w.id !== workspaceId),
        inspections: strip(get().inspections),
        records: strip(get().records),
        outbox: strip(get().outbox),
        contextBundles: strip(get().contextBundles),
        ...(leaving ? { view: { kind: "welcome" } as View } : {}),
      });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setSessionDefault(provider, patch) {
    const choice: ProviderChoice = { ...get().sessionDefaults[provider] };
    if (patch.model !== undefined) {
      if (patch.model) choice.model = patch.model;
      else delete choice.model;
    }
    if (patch.reasoningEffort !== undefined) {
      if (patch.reasoningEffort) choice.reasoningEffort = patch.reasoningEffort;
      else delete choice.reasoningEffort;
    }
    const defaults: SessionDefaults = { ...get().sessionDefaults, [provider]: choice };
    set({ sessionDefaults: defaults });
    try {
      await api.prefSet("session_defaults", defaults);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async resetSessionDefaults() {
    set({ sessionDefaults: {} });
    try {
      await api.prefSet("session_defaults", {});
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  noteImageFiles(sessionId, files, initial) {
    const seen = new Set(get().seenImageFiles[sessionId] ?? []);
    const fresh = files.filter((f) => !seen.has(f));
    if (fresh.length === 0) return;
    for (const f of fresh) seen.add(f);
    const observed = initial ? (get().observedImages[sessionId] ?? []) : [...(get().observedImages[sessionId] ?? []), ...fresh];
    set({ seenImageFiles: { ...get().seenImageFiles, [sessionId]: [...seen] }, observedImages: { ...get().observedImages, [sessionId]: observed } });
  },

  async checkForUpdate(reason) {
    if (get().appUpdate.checking || get().appUpdate.installing) return;
    set({ appUpdate: { ...get().appUpdate, checking: true, error: null } });
    try {
      const check = await api.appUpdateCheck();
      const remind = get().uiPrefs.updateReminder !== false;
      set({
        appUpdate: {
          ...get().appUpdate,
          current: check.current,
          available: check.available ?? null,
          checking: false,
          checkedAt: Date.now(),
          error: null,
          prompt: get().appUpdate.prompt || (reason === "startup" && remind && Boolean(check.available)),
        },
      });
    } catch (error) {
      // Offline, or no release published yet: said quietly by the button,
      // never as an error banner.
      set({ appUpdate: { ...get().appUpdate, checking: false, checkedAt: Date.now(), error: asError(error).message } });
    }
  },

  async installUpdate() {
    if (!get().appUpdate.available || get().appUpdate.installing) return;
    set({ appUpdate: { ...get().appUpdate, installing: { downloaded: 0, total: null }, error: null } });
    try {
      // On success the app restarts into the new version and this never returns.
      await api.appUpdateInstall((progress) => set({ appUpdate: { ...get().appUpdate, installing: { downloaded: progress.downloaded, total: progress.total ?? null } } }));
    } catch (error) {
      // The update was used up; a fresh check offers it again.
      set({ appUpdate: { ...get().appUpdate, installing: null, error: asError(error).message } });
      void get().checkForUpdate("manual");
    }
  },

  dismissUpdatePrompt(forever = false) {
    set({ appUpdate: { ...get().appUpdate, prompt: false } });
    if (forever) void get().setUiPrefs({ updateReminder: false });
  },

  async setUiPrefs(patch) {
    const next = { ...get().uiPrefs, ...patch };
    set({ uiPrefs: next });
    try {
      await api.prefSet("ui_prefs", next);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async exportTranscript(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    const lines: string[] = [`# Session ${sessionTitle(session)}`, "", `${PROVIDER_LABELS[session.snapshot.provider]} session id: ${session.snapshot.agentSessionId ?? session.handle.id}`, ""];
    for (const card of session.projection.cards) {
      if (card.kind === "message") {
        lines.push(`## ${card.message.role}`, "");
        for (const block of card.message.blocks) {
          if (block.type === "text") lines.push(block.text, "");
          else if (block.type === "resource_link") lines.push(`- reference: ${block.uri}`, "");
          else lines.push(`- ${block.type} block omitted`, "");
        }
      } else if (card.kind === "tool") {
        const patch = session.projection.toolCalls.get(card.toolCallId);
        lines.push(`- tool ${patch?.title ?? card.toolCallId} (${patch?.status ?? "no status"})`, "");
      } else if (card.kind === "notice") lines.push(`> ${card.title} ${card.description ?? ""}`, "");
      else if (card.kind === "turn" || card.kind === "runtime" || card.kind === "compaction") lines.push(`_${card.text}_`, "");
    }
    try {
      await api.saveTextFile(`transcript-${session.handle.id.slice(0, 12)}.md`, lines.join("\n"));
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async exportContinuity(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    set({ error: null });
    try {
      // This session's own conversation, as it stands: an export saves no
      // archive. In front of it, oldest first, the conversations it was
      // handed, read back from their archives one link at a time.
      const { markdown } = await continuityDocument(get, session);
      const earlier: string[] = [];
      let next = sessionContinuation(session, get().continuations)?.archive?.id ?? null;
      while (next && earlier.length < 32) {
        const id: string = next;
        const pages: string[] = [];
        next = null;
        try {
          for (let offset = 0; ;) {
            const page = await api.continuityRead(session.handle, id, offset, 24000);
            pages.push(page.text);
            next = page.archive.parent ?? null;
            if (page.nextOffset === null) break;
            if (page.nextOffset <= offset) throw new Error("The continuity reader returned a non-advancing page.");
            offset = page.nextOffset;
          }
        } catch {
          // An archive that is gone, or was changed, ends the line there.
          break;
        }
        earlier.unshift(pages.join(""));
      }
      await api.saveTextFile(`continuity-${session.handle.id.slice(0, 12)}.md`, [...earlier, markdown].join("\n\n---\n\n"));
    } catch (error) {
      set({ error: asError(error) });
      throw error;
    }
  },

  async openSession(workspaceId, mode, provider, launch) {
    if (mode.mode === "new" && provider === undefined && launch === undefined) {
      const preset = projectTeamPreset(get(), workspaceId);
      if (preset) return get().openSessionWithPreset(workspaceId, preset);
    }
    // Which program wrote this session decides which one can resume it. The
    // record knows, and the native side reads it again on a resume: launching
    // one provider against another's session id would ask the wrong program
    // for a session it has never heard of.
    const recorded = mode.mode === "resume" ? (get().records[workspaceId] ?? []).find((record) => record.agentSessionId === mode.session_id)?.provider : undefined;
    const defaults = get().sessionDefaults;
    // Without a project team or an explicit preset, new sessions use the default model.
    // A session the app is already running — a worker a delegation opened —
    // is shown, not resumed: a second process on the same session is refused
    // by the agent, and would compete with the first one if it were not.
    if (mode.mode === "resume") {
      const live = Object.values(get().sessions).find((session) => session.snapshot.agentSessionId === mode.session_id && session.projection.process !== "exited");
      if (live) return get().selectSession(live.handle.id);
    }
    const chosen: Provider = recorded ?? provider ?? startingProvider(get().providers);
    // A session opens its chat at once, and the agent starts behind it: a
    // new one, and one being resumed, which otherwise left the person
    // looking at the screen they came from, wondering if it had stuck.
    const message = launch?.message?.trim() ? launch.message : "";
    const record = mode.mode === "resume" ? (get().records[workspaceId] ?? []).find((entry) => entry.agentSessionId === mode.session_id) : undefined;
    const title = record?.titleOverlay || record?.autoTitle || undefined;
    const starting = {
      id: crypto.randomUUID(),
      workspaceId,
      provider: chosen,
      draft: message,
      queued: !!message || !!launch?.attachments?.length,
      ...(mode.mode === "resume" ? { resuming: true, ...(title ? { title } : {}) } : {}),
    };
    set({ starting, view: { kind: "starting" }, error: null, lockedResume: null });
    if (launch?.attachments?.length) get().addAttachments(starting.id, launch.attachments);
    // The chat gives way to the session once it is attached, unless the
    // person went elsewhere meanwhile; what was written carries over.
    const settle = (handleId: string) => {
      const waited = get().starting;
      if (waited?.id !== starting.id) return;
      if (get().view.kind === "starting") get().selectSession(handleId);
      const files = get().attachments[starting.id] ?? [];
      if (files.length) get().addAttachments(handleId, files);
      const { [starting.id]: _transferred, ...attachments } = get().attachments;
      set({ starting: null, attachments });
      if (waited.draft) {
        get().setDraft(handleId, waited.draft);
      }
      if (waited.queued && (waited.draft.trim() || files.length)) void get().send(handleId);
    };
    if (mode.mode === "resume") {
      try {
        const running = (await api.sessionListOpen()).find((entry) => entry.agentSessionId === mode.session_id);
        if (running) {
          await attachLive(get, set, running.handle, workspaceId, false, running.localSetup);
          settle(running.handle.id);
          return;
        }
      } catch {
        // Fall through to an ordinary resume.
      }
    }
    try {
      const opened = await api.sessionOpen({
        workspaceId,
        mode,
        provider: chosen,
        ...launchDefaults(defaults, chosen),
        ...(launch?.model ? { model: launch.model } : {}),
        ...(launch?.reasoningEffort ? { reasoningEffort: launch.reasoningEffort } : {}),
        ...(launch?.resources ? { resources: launch.resources } : {}),
        ...(launch?.combo ? { combo: launch.combo } : {}),
        ...(launch?.accountId ? { accountId: launch.accountId } : {}),
      });
      // Keep the preparation view visible through snapshot, subscription and
      // history loading. A half-attached session otherwise looks like a blank chat.
      await attachLive(get, set, opened.handle, workspaceId, false, opened.localSetup);
      settle(opened.handle.id);
    } catch (error) {
      const desktopError = asError(error);
      if (get().starting?.id === starting.id) set({ starting: { ...(get().starting ?? starting), error: desktopError.message } });
      if (mode.mode === "resume" && desktopError.code === "LOCKED") {
        set({ lockedResume: { workspaceId, sessionId: mode.session_id, error: desktopError } });
      }
      // A resume can retire the row (a session with no transcript); the list
      // should show that at once.
      if (mode.mode === "resume") void get().loadRecords(workspaceId);
      set({ error: desktopError });
    } finally {
      set({ busy: null });
    }
  },

  /**
   * Opens a fresh session on a named provider, for a caller that then has to
   * address it — the run moving to a new orchestrator, rather than a person
   * starting work.
   */
  async openSessionFor(workspaceId, provider, combo) {
    try {
      const opened = await api.sessionOpen({ workspaceId, mode: { mode: "new" }, provider, ...launchDefaults(get().sessionDefaults, provider), ...(combo ? { combo } : {}) });
      await attachLive(get, set, opened.handle, workspaceId, false, opened.localSetup);
      const live = get().sessions[opened.handle.id];
      const agentSessionId = live?.snapshot.agentSessionId ?? opened.snapshot.agentSessionId ?? null;
      return agentSessionId ? { key: opened.handle.id, agentSessionId } : null;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    }
  },

  async loadTeam(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      const [combo, jobs] = await Promise.all([api.delegationComboGet(session.handle), api.delegationJobs(session.handle)]);
      set({
        teams: combo ? { ...get().teams, [sessionId]: combo } : get().teams,
        jobs: { ...get().jobs, [sessionId]: jobs },
      });
    } catch {
      // A worker session, or delegation unavailable: no team to show.
    }
  },

  async saveTeam(sessionId, combo) {
    const session = get().sessions[sessionId];
    if (!session) return;
    // A Gemini session opened without a team has no team tools and cannot be
    // given them while it runs. The team is kept for it here, and with
    // helpers to lead the next message continues in a new Gemini session
    // that leads them, as a change of provider would.
    if (session.snapshot.provider === "gemini" && session.snapshot.leadsTeam === false) {
      set({ teams: { ...get().teams, [sessionId]: combo } });
      const options = asConfigOptions(session.projection.configOptions ?? session.snapshot.configOptions);
      const chosen = (value: unknown) => (typeof value === "string" && value !== "default" ? value : undefined);
      const model = chosen(options.find((option) => option.configId === "model")?.currentValue);
      const effort = chosen(options.find((option) => isEffortOption(option.configId))?.currentValue);
      get().setPendingProvider(sessionId, get().pendingProvider[sessionId] ?? { provider: "gemini", ...(model ? { model } : {}), ...(effort ? { effort } : {}) });
      return;
    }
    try {
      const saved = await api.delegationComboSet(session.handle, combo);
      set({ teams: { ...get().teams, [sessionId]: saved } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async loadDefaultCombo() {
    try {
      set({ defaultCombo: await api.delegationDefaultComboGet() });
    } catch {
      // Keeps the empty team.
    }
  },

  async saveDefaultCombo(combo) {
    try {
      set({ defaultCombo: await api.delegationDefaultComboSet(combo) });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async loadProviderModels(provider, fresh = false) {
    const known = get().providerModels[provider];
    if (known && !fresh) return known;
    const request = Symbol();
    modelRequests.set(provider, request);
    try {
      const models = (await api.providerModels(provider)).filter((model) => !isRetiredModel(provider, model));
      if (modelRequests.get(provider) !== request) return get().providerModels[provider] ?? [];
      set({ providerModels: { ...get().providerModels, [provider]: models } });
      return models;
    } catch {
      return [];
    }
  },

  async loadTeamChat(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return false;
    try {
      const view = await api.delegationChat(session.handle);
      if (!view || !Array.isArray(view.messages)) return false;
      // Asked for often while the chat is on screen: an unchanged answer
      // redraws nothing.
      if (JSON.stringify(get().teamChat[sessionId]) !== JSON.stringify(view)) set({ teamChat: { ...get().teamChat, [sessionId]: view } });
      return true;
    } catch {
      return false;
    }
  },

  async checkTeamChat(sessionId, token) {
    const known = () => get().teamChat[sessionId]?.received?.some((receipt) => receipt.token === token) ?? false;
    const check = `${sessionId}|${token}`;
    if (known() || get().teamChatUnknown[sessionId]?.includes(token) || teamChatChecks.has(check)) return;
    teamChatChecks.add(check);
    try {
      // The host keeps a receipt before it writes the prompt, so one read
      // settles it: there, or never written by the host.
      await get().loadTeamChat(sessionId);
      if (!known()) set({ teamChatUnknown: { ...get().teamChatUnknown, [sessionId]: [...(get().teamChatUnknown[sessionId] ?? []), token] } });
    } finally {
      teamChatChecks.delete(check);
    }
  },

  noteJob(job) {
    const previous = get().jobs[job.orchestrator]?.find((existing) => existing.id === job.id);
    set({ jobs: { ...get().jobs, [job.orchestrator]: upsertJob(get().jobs[job.orchestrator], job) } });
    // A worker closed for being idle is archived by the host: the sidebar
    // reads the rows again so it leaves the live list.
    if (job.workerClosed && !previous?.workerClosed) {
      const workspaceId = get().sessions[job.orchestrator]?.workspaceId;
      if (workspaceId) void get().loadRecords(workspaceId);
    }
    // A new worker session has a row now: the sidebar shows it under its
    // orchestrator.
    if (job.workerSession && previous?.workerSession !== job.workerSession) {
      const workspaceId = get().sessions[job.orchestrator]?.workspaceId;
      if (workspaceId) {
        void get().loadRecords(workspaceId);
        // Attached in the background, so its row is live and opening it
        // shows the running worker rather than trying to resume it.
        const handleId = job.workerSession;
        if (!get().sessions[handleId]) {
          void api
            .sessionListOpen()
            .then((open) => open.find((entry) => entry.handle.id === handleId))
            .then((entry) => (entry && !get().sessions[handleId] ? attachLive(get, set, entry.handle, workspaceId, false, entry.localSetup) : undefined))
            .catch(() => undefined);
        }
      }
    }
  },

  async cancelJob(sessionId, jobId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      get().noteJob(await api.delegationJobCancel(session.handle, jobId));
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async retryJob(sessionId, jobId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      get().noteJob(await api.delegationJobRetry(session.handle, jobId));
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async loadUltraReviews(host) {
    try {
      const reviews = await api.ultraReviewList(host);
      set({ ultraReviews: { ...get().ultraReviews, [host]: reviews } });
    } catch {
      // Ultra-Review is unavailable (no team service): the tab says so on its own.
    }
  },

  noteUltraReview(review) {
    const list = get().ultraReviews[review.host] ?? [];
    const next = list.some((entry) => entry.id === review.id) ? list.map((entry) => (entry.id === review.id ? review : entry)) : [review, ...list];
    const previous = list.find((entry) => entry.id === review.id);
    set({ ultraReviews: { ...get().ultraReviews, [review.host]: next } });
    // Like any finished turn: the kind of event and the folder, never content.
    const settings = get().settings;
    if (previous?.status === "running" && review.status === "done" && settings && shouldNotify(settings, "completed", review.workspaceId)) {
      const workspace = get().workspaces.find((entry) => entry.id === review.workspaceId);
      void notify("completed", workspace ? basenameOf(workspace.displayPath) : t("a workspace"));
    }
  },

  async startUltraReview(request) {
    set({ busy: t("Checking out the change for the reviewers"), error: null });
    try {
      get().noteUltraReview(await api.ultraReviewStart(request));
      return true;
    } catch (error) {
      set({ error: asError(error) });
      return false;
    } finally {
      set({ busy: null });
    }
  },

  async cancelUltraReview(id) {
    try {
      await api.ultraReviewCancel(id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async resumeUltraReview(id) {
    try {
      get().noteUltraReview(await api.ultraReviewResume(id));
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async noteOnFinding(id, finding, text) {
    try {
      get().noteUltraReview(await api.ultraReviewNote(id, finding, text));
      return true;
    } catch (error) {
      set({ error: asError(error) });
      return false;
    }
  },

  async decideFinding(id, finding, decision) {
    try {
      get().noteUltraReview(await api.ultraReviewDecide(id, finding, decision));
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async deleteUltraReview(review) {
    try {
      await api.ultraReviewDelete(review.id);
      set({ ultraReviews: { ...get().ultraReviews, [review.host]: (get().ultraReviews[review.host] ?? []).filter((entry) => entry.id !== review.id) } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async openWorker(job) {
    const handleId = job.workerSession;
    if (!handleId) return;
    if (!get().sessions[handleId]) {
      try {
        const entry = (await api.sessionListOpen()).find((open) => open.handle.id === handleId);
        const workspaceId = get().sessions[job.orchestrator]?.workspaceId ?? entry?.workspaceId ?? undefined;
        if (!entry || !workspaceId) {
          set({ error: { code: "NOT_READY", message: t("This worker has stopped. Its transcript is in the sidebar under its orchestrator."), retry: "user_action" } });
          return;
        }
        await attachLive(get, set, entry.handle, workspaceId, true, entry.localSetup);
        return;
      } catch (error) {
        set({ error: asError(error) });
        return;
      }
    }
    get().selectSession(handleId);
  },

  async handOff(sessionId, provider, preset) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId)) return;
    leaderContinuations.add(sessionId);
    let target: SessionHandle | null = null;
    try {
      const session = idleHandoffSource(get, sessionId);
      await validateLeaderSource(get, session);
      const context = await handoffContext(get, session);
      const sequence = context.sequence;
      await validateLeaderSource(get, session, sequence);
      const team = preset?.combo ?? get().teams[sessionId];
      const opened = await api.sessionOpen({
          workspaceId: session.workspaceId,
          mode: { mode: "new" },
          provider: preset?.orchestrator.provider ?? provider,
          ...launchDefaults(get().sessionDefaults, preset?.orchestrator.provider ?? provider),
          ...(preset?.orchestrator.resources ? { resources: preset.orchestrator.resources } : {}),
          ...(preset?.orchestrator.model ? { model: preset.orchestrator.model } : {}),
          ...(preset?.orchestrator.effort ? { reasoningEffort: preset.orchestrator.effort } : {}),
          ...(team ? { combo: team } : {}),
      });
      target = opened.handle;
      await attachLive(get, set, target, session.workspaceId, false, opened.localSetup);
      await validateLeaderSource(get, session, sequence);
      const { brief, archive } = await bindArchive(session, get().sessions[target.id], context);
      const agent = get().sessions[target.id]?.snapshot.agentSessionId;
      if (agent && session.snapshot.agentSessionId) {
        await keepPendingBrief(agent, brief);
        await saveContinuation(get, set, agent, { fromAgentSessionId: session.snapshot.agentSessionId, fromProvider: session.snapshot.provider, target: continuationTarget(get().sessions[target.id]!), briefPending: true, ...(archive ? { archive } : {}) });
      }
      await validateLeaderSource(get, session, sequence);
      const key = target.id;
      set({ drafts: { ...get().drafts, [key]: t("Continue from where the previous session stopped.") }, sideBrief: { ...get().sideBrief, [key]: brief }, ...(team ? { teams: { ...get().teams, [key]: team } } : {}), error: null });
      target = null;
      get().selectSession(key);
    } catch (error) {
      if (target) await discardContinuation(get, set, target);
      set({ error: asError(error) });
    } finally {
      leaderContinuations.delete(sessionId);
    }
  },

  async prepareQuotaContinuation(sessionId, target) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId) || quotaContinuationProviders.has(target.provider)) return null;
    quotaContinuationSessions.add(sessionId);
    quotaContinuationProviders.add(target.provider);
    let opened: SessionHandle | null = null;
    function refuse(message: string): never { throw { code: "NOT_READY", message, retry: "user_action" }; }
    const checkSource = () => {
      const session = get().sessions[sessionId];
      if (!session || session.projection.attachment !== "attached" || session.projection.process === "exited") refuse(t("The source session changed while the continuation was opening. Try again."));
      if (!session!.projection.quotaFailure) refuse(t("This session has not reported an exhausted usage limit."));
      const projection = session!.projection;
      if (session!.inFlightRequestId || session!.pendingPrompt || session!.steerInFlight
        || ["running", "awaiting_user", "cancelling"].includes(projection.foreground)
        || projection.autonomous.size > 0
        || [...projection.detached.values()].some((state) => state === "active" || state === "cancellation_requested")
        || [...projection.background.values()].some((task) => task.state === "running")
        || [...projection.inspector.agents.values()].some((agent) => agent.status === "working" || agent.status === "starting")
        || (get().jobs[sessionId] ?? []).some((job) => ["starting", "running", "waiting"].includes(job.status))) refuse(t("Wait for this session and its workers to finish before preparing a continuation."));
      if (projection.queue.items.length || projection.queue.sending) refuse(t("Resolve or remove the queued messages before preparing a continuation."));
      const goal = get().odyssey[sessionId]?.goal;
      if (goal && goal.state !== "complete" && goal.state !== "abandoned") refuse(t("Move this goal from the Big Thing tab so its progress follows it."));
      return session!;
    };
    const checkSnapshot = (snapshot: Snapshot) => {
      if (["running", "awaiting_user", "cancelling"].includes(snapshot.foreground) || snapshot.autonomousTurns.length
        || Object.values(snapshot.detachedCalls).some((state) => state === "active" || state === "cancellation_requested")
        || snapshot.backgroundTasks?.some((task) => task.state === "running")) refuse(t("Wait for this session and its workers to finish before preparing a continuation."));
      if (snapshot.queue?.items.length || snapshot.queue?.sending) refuse(t("Resolve or remove the queued messages before preparing a continuation."));
    };
    try {
      const source = checkSource();
      const sourceAgent = source.snapshot.agentSessionId;
      if (!sourceAgent || !ACCOUNT_PROVIDERS.includes(target.provider)) refuse(t("Choose another signed-in account or provider."));
      const [accounts, snapshot, jobs, combo, goal] = await Promise.all([
        api.accountsList(target.provider), api.sessionSnapshot(source.handle), api.delegationJobs(source.handle),
        api.delegationComboGet(source.handle), api.odysseyForSession(source.workspaceId, sourceAgent!),
      ]);
      checkSource();
      checkSnapshot(snapshot);
      if (jobs.some((job) => ["starting", "running", "waiting"].includes(job.status))) refuse(t("Wait for this session and its workers to finish before preparing a continuation."));
      if (goal && goal.goal.state !== "complete" && goal.goal.state !== "abandoned") refuse(t("Move this goal from the Big Thing tab so its progress follows it."));
      const account = target.accountId ? accounts.find((entry) => entry.id === target.accountId) : accounts.find((entry) => entry.active);
      const provider = get().providers?.find((entry) => entry.provider === target.provider);
      if (!provider?.resolved || !account || account.provider !== target.provider || account.auth?.loggedIn !== true || account.auth.setupRequired
        || (snapshot.provider === target.provider && (snapshot.accountId ?? "default") === account.id)) refuse(t("Choose another signed-in account or provider."));
      const options = asConfigOptions(source.projection.configOptions ?? source.snapshot.configOptions);
      const configValue = (id: string) => {
        const value = options.find((option) => option.configId === id)?.currentValue;
        return typeof value === "string" && value !== "default" ? value : undefined;
      };
      const model = snapshot.provider === target.provider ? configValue("model") : undefined;
      const effort = snapshot.provider === target.provider ? options.find((option) => isEffortOption(option.configId))?.currentValue : undefined;
      const response = await api.sessionOpen({
        workspaceId: source.workspaceId, mode: { mode: "new" }, provider: target.provider, accountId: account.id,
        ...launchDefaults(get().sessionDefaults, target.provider),
        ...(model ? { model } : {}), ...(typeof effort === "string" ? { reasoningEffort: effort } : {}), combo: combo ?? EMPTY_COMBO,
      });
      opened = response.handle;
      // An older host must not silently ignore the selected account.
      if (response.snapshot.provider !== target.provider || response.snapshot.accountId !== account.id) refuse(t("Choose another signed-in account or provider."));
      await attachLive(get, set, opened, source.workspaceId, false, response.localSetup);
      const current = checkSource();
      checkSnapshot(await api.sessionSnapshot(source.handle));
      checkSource();
      const destination = get().sessions[opened.id];
      if (!destination?.snapshot.agentSessionId || destination.snapshot.accountId !== account.id) refuse(t("The source session changed while the continuation was opening. Try again."));
      const context = await handoffContext(get, current, 24_000);
      const sequence = context.sequence;
      checkSource();
      const { brief, archive } = await bindArchive(source, destination, context);
      const continuation: Continuation = { fromAgentSessionId: sourceAgent!, fromProvider: snapshot.provider, target: continuationTarget(destination), briefPending: true, ...(archive ? { archive } : {}) };
      const destinationAgent = destination.snapshot.agentSessionId;
      await keepPendingBrief(destinationAgent, brief);
      await saveContinuation(get, set, destinationAgent, continuation);
      // Copy the latest composer state, including edits made during startup.
      checkSource();
      await validateLeaderSource(get, source, sequence);
      set({
        drafts: { ...get().drafts, [opened.id]: get().drafts[sessionId]?.length ? get().drafts[sessionId]! : t("Continue from where the previous session stopped.") },
        attachments: { ...get().attachments, [opened.id]: [...(get().attachments[sessionId] ?? [])] },
        mentions: { ...get().mentions, [opened.id]: [...(get().mentions[sessionId] ?? [])] },
        teams: { ...get().teams, [opened.id]: combo ?? EMPTY_COMBO },
        sideBrief: { ...get().sideBrief, [opened.id]: brief },
        continuations: { ...get().continuations, [destinationAgent]: continuation },
        continuationHistory: { ...get().continuationHistory, [opened.id]: [...current.projection.cards] }, error: null,
      });
      const key = opened.id;
      opened = null;
      get().selectSession(key);
      void get().renameSession(source.workspaceId, destinationAgent, sessionTitle(current));
      return key;
    } catch (error) {
      if (opened) {
        try {
          await api.sessionStop(opened);
          const { [opened.id]: _unused, ...sessions } = get().sessions;
          set({ sessions, sessionOrder: get().sessionOrder.filter((key) => key !== opened!.id) });
        } catch { /* Leave a target that could not be stopped visible. The source is untouched. */ }
      }
      set({ error: asError(error) });
      return null;
    } finally {
      quotaContinuationSessions.delete(sessionId);
      quotaContinuationProviders.delete(target.provider);
    }
  },

  async forkSession(source, point, how) {
    const { workspaceId, agentSessionId, provider } = source;
    const live = (source.key ? get().sessions[source.key] : undefined) ?? Object.values(get().sessions).find((session) => session.snapshot.agentSessionId === agentSessionId);
    const records = get().records[workspaceId] ?? [];
    const record = records.find((entry) => entry.agentSessionId === agentSessionId);
    const base = live ? sessionTitle(live, records) : record?.titleOverlay || record?.autoTitle || t("Session");
    // A side chat is a fork that can also act on this session's goal.
    const side = Boolean(how?.side && live);
    const title = forkTitle(base, records.map((entry) => entry.titleOverlay || entry.autoTitle || ""), "fork");
    // The copy works the way the original does: its model, effort and team.
    const options = live ? asConfigOptions(live.snapshot.configOptions) : [];
    const chosen = (option: { currentValue?: unknown } | undefined) => (typeof option?.currentValue === "string" && option.currentValue !== "default" ? option.currentValue : undefined);
    const localSetup = provider === "local" ? live?.localSetup : undefined;
    const model = localSetup?.model ?? chosen(options.find((option) => option.configId === "model"));
    const effort = chosen(options.find((option) => isEffortOption(option.configId)));
    const team = live ? get().teams[live.handle.id] : undefined;
    const cards = live?.projection.cards;
    const copies = copiesItself(provider, point, cards);
    if (!copies && !point?.fresh && !cards) {
      set({ error: { code: "NOT_READY", message: t("Open this session first: {provider} cannot copy a session itself, so the fork starts from a summary of the open conversation.", { provider: PROVIDER_LABELS[provider] }), retry: "user_action" } });
      return null;
    }
    set({ busy: t("Forking “{title}”", { title: base }), error: null });
    try {
      // Into a worktree: a branch of its own from where the project is now,
      // uncommitted changes carried over, as a workspace of its own.
      let target = workspaceId;
      if (how?.worktree) {
        const created = await api.worktreeCreate(workspaceId, forkBranch(title), "HEAD", true);
        const inspection = await api.workspaceInspect(created.workspace.displayPath);
        set({ workspaces: await api.workspaceList(), inspections: { ...get().inspections, [inspection.record.id]: inspection } });
        target = inspection.record.id;
      }
      const launch = { workspaceId: target, forkSource: { workspaceId, agentSessionId }, provider, ...(localSetup?.resources ? { resources: localSetup.resources } : {}), ...(model ? { model } : {}), ...(effort ? { reasoningEffort: effort } : {}), ...(team ? { combo: team } : {}), ...(side && live ? { sideOf: live.handle.id } : {}) };
      const opened = copies
        ? await api.sessionOpen({ ...launch, mode: { mode: "fork", session_id: agentSessionId }, ...(provider === "claude" && point?.at ? { forkAt: point.at } : {}) })
        : await api.sessionOpen({ ...launch, mode: { mode: "new" } });
      await attachLive(get, set, opened.handle, target, false, opened.localSetup);
      const key = opened.handle.id;
      const forked = get().sessions[key]?.snapshot.agentSessionId ?? opened.snapshot.agentSessionId;
      if (!forked) throw new Error(t("The agent did not name the new session."));
      // A copy by brief: what was said up to the point rides, unseen, in
      // front of the first message, as a side session's does.
      if (!copies && !point?.fresh && cards) {
        const brief = handoffBrief(cards.slice(0, (point?.upTo ?? cards.length - 1) + 1), provider);
        set({ sideBrief: { ...get().sideBrief, [key]: brief } });
      }
      // Nested where it is listed with its source: a worktree's sessions are
      // listed under the worktree.
      if (target === workspaceId) await api.sessionNest(workspaceId, forked, agentSessionId);
      await get().renameSession(target, forked, title);
      if (point?.draft) set({ drafts: { ...get().drafts, [key]: point.draft } });
      await get().loadRecords(target);
      get().selectSession(key);
      return key;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    } finally {
      set({ busy: null });
    }
  },

  async loadNesting(workspaceId, agentSessionId) {
    try {
      const [reports, reporting] = await Promise.all([api.sessionNestedReports(workspaceId, agentSessionId), api.sessionReporting(workspaceId, agentSessionId)]);
      set({ nestedReports: { ...get().nestedReports, [agentSessionId]: reports }, reporting: { ...get().reporting, [agentSessionId]: reporting } });
    } catch {
      // Not listed yet: nothing waits for it, and it is no fork.
    }
  },

  async setReporting(workspaceId, agentSessionId, on) {
    try {
      await api.sessionSetReporting(workspaceId, agentSessionId, on);
      set({ reporting: { ...get().reporting, [agentSessionId]: on } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async nestSession(workspaceId, agentSessionId, parentAgentSessionId) {
    try {
      await api.sessionNest(workspaceId, agentSessionId, parentAgentSessionId);
      await get().loadRecords(workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  setPendingProvider(sessionId, pending) {
    const { [sessionId]: _previous, ...rest } = get().pendingProvider;
    set({ pendingProvider: pending && switchesSession(get().sessions[sessionId], pending, get().teams[sessionId]) ? { ...rest, [sessionId]: pending } : rest });
  },

  async loadContinuations() {
    try {
      const stored = await api.prefGet<Record<string, Continuation>>(CONTINUATIONS_KEY);
      if (stored && typeof stored === "object") {
        set({ continuations: stored });
        // Sessions attached before the records arrived get their waiting brief now.
        for (const key of Object.keys(get().sessions)) void restorePendingBrief(get, set, key);
      }
    } catch {
      // None yet.
    }
  },

  async continueOnProvider(sessionId) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId)) return;
    const session = get().sessions[sessionId];
    const pending = get().pendingProvider[sessionId];
    const draft = get().drafts[sessionId] ?? "";
    const attachments = get().attachments[sessionId] ?? [];
    const mentions = get().mentions[sessionId] ?? [];
    if (!session || !pending || (!draft.trim() && attachments.length === 0 && mentions.length === 0) || session.inFlightRequestId) return;
    const from = session.snapshot.provider;
    const fromAgentSessionId = session.snapshot.agentSessionId;
    const title = sessionTitle(session);
    const team = get().teams[sessionId];
    // Nothing said yet: nothing to hand off. The session simply starts on
    // the chosen provider with the person's message.
    const fresh = !hasConversation(session.projection.cards) && !get().sideBrief[sessionId]
      && !sessionContinuation(session, get().continuations);
    const provider = PROVIDER_LABELS[pending.provider];
    // The model the chip shows when none was picked: the provider's default,
    // else its first. Local Models has no account default to fall back on.
    const catalog = get().providerModels[pending.provider] ?? [];
    const shownModel = pending.model ?? (catalog.find((entry) => entry.isDefault) ?? catalog[0])?.id;
    const model = pending.provider === "local" ? shownModel : pending.model;
    leaderContinuations.add(sessionId);
    let target: SessionHandle | null = null;
    let submitStarted = false;
    set({ busy: fresh ? t("Starting with {provider}", { provider }) : t("Continuing with {provider}", { provider }), error: null });
    try {
      await validateLeaderSource(get, session);
      const context = fresh ? undefined : await handoffContext(get, session);
      await validateLeaderSource(get, session, context?.sequence);
      const opened = await api.sessionOpen({
        workspaceId: session.workspaceId,
        mode: { mode: "new" },
        provider: pending.provider,
        ...launchDefaults(get().sessionDefaults, pending.provider),
        ...(model ? { model } : {}),
        ...(pending.resources ? { resources: pending.resources } : {}),
        ...(pending.effort ? { reasoningEffort: pending.effort } : {}),
        ...(team ? { combo: team } : {}),
      });
      target = opened.handle;
      await attachLive(get, set, opened.handle, session.workspaceId, false, opened.localSetup);
      const key = opened.handle.id;
      const live = get().sessions[key];
      const agentSessionId = live?.snapshot.agentSessionId ?? opened.snapshot.agentSessionId ?? null;
      await validateLeaderSource(get, session, context?.sequence);
      const bound = context ? await bindArchive(session, live, context) : undefined;
      await validateLeaderSource(get, session, context?.sequence);
      if (!fresh && agentSessionId && fromAgentSessionId) {
        await saveContinuation(get, set, agentSessionId, { fromAgentSessionId, fromProvider: from, target: continuationTarget(live!), ...(bound?.archive ? { archive: bound.archive } : {}) });
      }
      await validateLeaderSource(get, session, context?.sequence);
      // The brief rides in front of the person's message; the transcript
      // shows only the message (see withoutHandoff).
      const prompt = bound ? `${bound.brief}${HANDOFF_SPLIT}${draft}` : draft;
      submitStarted = true;
      const response = await api.sessionSubmit(opened.handle, crypto.randomUUID(), prompt, attachments.map((a) => a.id), mentions);
      if (response.outcome.outcome !== "accepted") {
        // Nothing changes for the person: the draft stays, and the session
        // opened for it goes away.
        await discardContinuation(get, set, opened.handle);
        target = null;
        set({ error: response.outcome.error });
        return;
      }
      target = null; // Accepted work must stay visible, even if later housekeeping fails.
      const composerUnchanged = (get().drafts[sessionId] ?? "") === draft
        && JSON.stringify(get().attachments[sessionId] ?? []) === JSON.stringify(attachments)
        && JSON.stringify(get().mentions[sessionId] ?? []) === JSON.stringify(mentions);
      const { [sessionId]: _done, ...pendingRest } = get().pendingProvider;
      set({
        ...(composerUnchanged ? {
          drafts: { ...get().drafts, [sessionId]: "" },
          attachments: { ...get().attachments, [sessionId]: [] },
          mentions: { ...get().mentions, [sessionId]: [] },
        } : {}),
        pendingProvider: pendingRest,
        ...(fresh ? {} : { continuationHistory: { ...get().continuationHistory, [key]: session.projection.cards } }),
      });
      if (!fresh && agentSessionId && fromAgentSessionId) {
        // One conversation in the list: the new session keeps the title,
        // and the earlier one is archived (still there among archived).
        await get().renameSession(session.workspaceId, agentSessionId, title);
      }
      get().selectSession(key);
      if (fromAgentSessionId && composerUnchanged) {
        await api.sessionStop(session.handle).catch(() => undefined);
        const { [sessionId]: _stopped, ...rest } = get().sessions;
        set({ sessions: rest, sessionOrder: get().sessionOrder.filter((id) => id !== sessionId) });
        await get().archiveSession(session.workspaceId, fromAgentSessionId, true);
      }
    } catch (error) {
      // An IPC error after submit may have accepted work. Leave that target
      // visible and keep the source; durable outbox reconciliation owns it.
      if (target && !submitStarted) await discardContinuation(get, set, target);
      set({ error: asError(error) });
    } finally {
      leaderContinuations.delete(sessionId);
      set({ busy: null });
    }
  },

  async loadTeamPresets() {
    try {
      const previousProjectTeams = get().projectTeamPresetIds;
      const [presets, defaultId, savedProjectTeams] = await Promise.all([
        api.prefGet<unknown>(TEAM_PRESETS_KEY),
        api.prefGet<string>(DEFAULT_PRESET_KEY),
        api.prefGet<unknown>(PROJECT_TEAM_PRESETS_KEY).catch(() => undefined),
      ]);
      const projectTeams = savedProjectTeams && typeof savedProjectTeams === "object" && !Array.isArray(savedProjectTeams)
        ? Object.fromEntries(Object.entries(savedProjectTeams).filter((entry): entry is [string, string] => typeof entry[1] === "string"))
        : {};
      set((state) => ({
        teamPresets: readPresets(presets),
        defaultPresetId: null,
        ...(savedProjectTeams !== undefined && state.projectTeamPresetIds === previousProjectTeams ? { projectTeamPresetIds: projectTeams } : {}),
      }));
      // Clear the older global default and the host's default workers it implied.
      if (typeof defaultId === "string" && defaultId) void get().setDefaultPreset(null);
    } catch {
      // No presets yet.
    }
  },

  setProjectTeamPreset(workspaceId, presetId) {
    // Each write starts from the last saved map, including changes to other projects.
    projectTeamWrite = projectTeamWrite.then(async () => {
      const projectTeamPresetIds = { ...get().projectTeamPresetIds };
      if (presetId === null) delete projectTeamPresetIds[workspaceId];
      else if (get().teamPresets.some((preset) => preset.id === presetId)) projectTeamPresetIds[workspaceId] = presetId;
      else return;
      await api.prefSet(PROJECT_TEAM_PRESETS_KEY, projectTeamPresetIds);
      set({ projectTeamPresetIds });
    }).catch((error) => set({ error: asError(error) }));
    return projectTeamWrite;
  },

  async saveTeamPreset(preset) {
    const saved = { ...preset, name: preset.name.trim() || "Team", updatedAt: Date.now() };
    const list = upsertPreset(get().teamPresets, saved);
    try {
      await api.prefSet(TEAM_PRESETS_KEY, list);
      set({ teamPresets: list });
      // The default preset's workers are what the host starts a session with.
      if (get().defaultPresetId === saved.id) await get().saveDefaultCombo(saved.combo);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async loadHelperPresets() {
    try {
      set({ helperPresets: readHelperPresets(await api.prefGet<unknown>(HELPER_PRESETS_KEY)) });
    } catch {
      // None saved yet.
    }
  },

  async saveHelperPreset(worker) {
    const preset = helperPresetFrom(worker, get().helperPresets);
    const list = upsertHelperPreset(get().helperPresets, preset);
    try {
      await api.prefSet(HELPER_PRESETS_KEY, list);
      set({ helperPresets: list });
      return preset;
    } catch (error) {
      set({ error: asError(error) });
      return null;
    }
  },

  async addHelperPresets(presets) {
    const list = presets.reduce((all, preset) => upsertHelperPreset(all, preset), get().helperPresets);
    try {
      await api.prefSet(HELPER_PRESETS_KEY, list);
      set({ helperPresets: list });
      return true;
    } catch (error) {
      set({ error: asError(error) });
      return false;
    }
  },

  async deleteHelperPreset(id) {
    const list = get().helperPresets.filter((preset) => preset.id !== id);
    try {
      await api.prefSet(HELPER_PRESETS_KEY, list);
      set({ helperPresets: list });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async addTeamPresets(presets) {
    const list = presets.reduce((all, preset) => upsertPreset(all, preset), get().teamPresets);
    try {
      await api.prefSet(TEAM_PRESETS_KEY, list);
      set({ teamPresets: list });
      return true;
    } catch (error) {
      set({ error: asError(error) });
      return false;
    }
  },

  async deleteTeamPreset(id) {
    const list = get().teamPresets.filter((preset) => preset.id !== id);
    try {
      await api.prefSet(TEAM_PRESETS_KEY, list);
      set({ teamPresets: list });
      if (get().defaultPresetId === id) await get().setDefaultPreset(null);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setDefaultPreset(id) {
    const preset = get().teamPresets.find((entry) => entry.id === id) ?? null;
    try {
      await api.prefSet(DEFAULT_PRESET_KEY, preset ? preset.id : null);
      set({ defaultPresetId: preset ? preset.id : null });
      await get().saveDefaultCombo(preset ? preset.combo : EMPTY_COMBO);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async openSessionWithPreset(workspaceId, preset, message, attachments) {
    const { provider, model, effort, resources } = preset.orchestrator;
    await get().openSession(workspaceId, { mode: "new" }, provider, {
      ...(model ? { model } : {}),
      ...(effort ? { reasoningEffort: effort } : {}),
      combo: preset.combo,
      ...(resources ? { resources } : {}),
      ...(message ? { message } : {}),
      ...(attachments?.length ? { attachments } : {}),
    });
  },

  async applyPresetToSession(sessionId, preset) {
    const session = get().sessions[sessionId];
    if (!session) return "applied";
    if (switchesSession(session, preset.orchestrator, preset.combo)) return "needs_handoff";
    await get().saveTeam(sessionId, preset.combo);
    const options = asConfigOptions(session.snapshot.configOptions);
    const model = options.find((option) => option.configId === "model");
    const effort = options.find((option) => isEffortOption(option.configId));
    if (!localAgentHarness(preset.orchestrator.model) && preset.orchestrator.model && model && model.currentValue !== preset.orchestrator.model) {
      await get().setConfigOption(sessionId, model.configId, preset.orchestrator.model);
    }
    if (preset.orchestrator.effort && effort && effort.currentValue !== preset.orchestrator.effort) {
      await get().setConfigOption(sessionId, effort.configId, preset.orchestrator.effort);
    }
    return "applied";
  },

  selectSession(sessionId) {
    if (!get().sessions[sessionId]) return;
    set({ sessionTab: "transcript", transcriptSearchOpen: false });
    get().setView({ kind: "session", sessionId });
    get().focusComposer();
  },

  setDraft(sessionId, draft) {
    set({ drafts: { ...get().drafts, [sessionId]: draft } });
  },

  async send(sessionId) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId)) return;
    const pending = get().pendingProvider[sessionId];
    if (pending && switchesSession(get().sessions[sessionId], pending, get().teams[sessionId])) return get().continueOnProvider(sessionId);
    const session = get().sessions[sessionId];
    const draft = get().drafts[sessionId] ?? "";
    const attachments = get().attachments[sessionId] ?? [];
    const mentions = get().mentions[sessionId] ?? [];
    if (!session || (!draft.trim() && attachments.length === 0 && mentions.length === 0) || session.inFlightRequestId) return;
    const requestId = crypto.randomUUID();
    const pendingPrompt = { requestId, text: draft, attachments, mentions, queued: false, accepted: false };
    const started = performance.now();
    set({
      sessions: { ...get().sessions, [sessionId]: { ...session, inFlightRequestId: requestId, pendingPrompt } },
      drafts: { ...get().drafts, [sessionId]: "" },
      attachments: { ...get().attachments, [sessionId]: [] },
      mentions: { ...get().mentions, [sessionId]: [] },
      error: null,
    });
    try {
      const brief = get().sideBrief[sessionId];
      const response = await api.sessionSubmit(
        session.handle,
        requestId,
        brief ? `${brief}${HANDOFF_SPLIT}${draft}` : draft,
        attachments.map((a) => a.id),
        mentions,
      );
      if (response.outcome.outcome === "accepted") {
        if (brief) {
          const { [sessionId]: _sent, ...rest } = get().sideBrief;
          set({ sideBrief: rest });
          consumeContinuationBrief(get, set, sessionId);
        }
        reportsSent(get, set, session.snapshot.agentSessionId);
        const current = get().sessions[sessionId];
        if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, pendingPrompt: current.pendingPrompt?.requestId === requestId && !promptPublished(current.projection, requestId) ? { ...pendingPrompt, accepted: true } : undefined } } });
      } else {
        set({ ...restorePrompt(get(), sessionId, pendingPrompt), error: response.outcome.error });
      }
    } catch (error) {
      set({ ...restorePrompt(get(), sessionId, pendingPrompt), error: asError(error) });
    } finally {
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, inFlightRequestId: null, pendingPrompt: current.pendingPrompt?.accepted ? current.pendingPrompt : undefined } } });
      console.debug("session send", { requestId, elapsedMs: Math.round(performance.now() - started) });
    }
  },

  async steer(sessionId) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId)) return;
    const session = get().sessions[sessionId];
    const draft = get().drafts[sessionId] ?? "";
    if (!session || !draft.trim() || session.steerInFlight) return;
    set({ sessions: { ...get().sessions, [sessionId]: { ...session, steerInFlight: true } }, error: null });
    try {
      const outcome = await api.sessionSteer(session.handle, draft);
      if (outcome.outcome === "accepted") set({ drafts: { ...get().drafts, [sessionId]: "" } });
      else set({ error: outcome.error });
    } catch (error) {
      set({ error: asError(error) });
    } finally {
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, steerInFlight: false } } });
    }
  },

  async enqueue(sessionId) {
    if (leaderContinuations.has(sessionId) || quotaContinuationSessions.has(sessionId)) return;
    const session = get().sessions[sessionId];
    const draft = get().drafts[sessionId] ?? "";
    const attachments = get().attachments[sessionId] ?? [];
    const mentions = get().mentions[sessionId] ?? [];
    if (!session || (!draft.trim() && attachments.length === 0 && mentions.length === 0) || session.inFlightRequestId) return;
    const requestId = crypto.randomUUID();
    const pendingPrompt = { requestId, text: draft, attachments, mentions, queued: true, accepted: false };
    set({
      sessions: { ...get().sessions, [sessionId]: { ...session, inFlightRequestId: requestId, pendingPrompt } },
      drafts: { ...get().drafts, [sessionId]: "" },
      attachments: { ...get().attachments, [sessionId]: [] },
      mentions: { ...get().mentions, [sessionId]: [] },
      error: null,
    });
    try {
      // The composer's own state rides along, so taking the prompt back out
      // to edit restores its chips too, from this window or the phone.
      const brief = get().sideBrief[sessionId];
      const restoreAttachments = attachments.map((attachment) => attachment.document
        ? { ...attachment, document: { ...attachment.document, text: "" } }
        : attachment);
      await api.sessionEnqueue(session.handle, requestId, brief ? `${brief}${HANDOFF_SPLIT}${draft}` : draft, attachments.map((a) => a.id), mentions, { attachments: restoreAttachments, mentions });
      if (brief) {
        const { [sessionId]: _sent, ...rest } = get().sideBrief;
        set({ sideBrief: rest });
        consumeContinuationBrief(get, set, sessionId);
      }
      reportsSent(get, set, session.snapshot.agentSessionId);
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, pendingPrompt: current.pendingPrompt?.requestId === requestId && !promptPublished(current.projection, requestId) ? { ...pendingPrompt, accepted: true } : undefined } } });
    } catch (error) {
      set({ ...restorePrompt(get(), sessionId, pendingPrompt), error: asError(error) });
    } finally {
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, inFlightRequestId: null, pendingPrompt: current.pendingPrompt?.accepted ? current.pendingPrompt : undefined } } });
    }
  },

  async queueOp(sessionId, op) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      await api.sessionQueue(session.handle, op);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async editQueued(sessionId, id) {
    const session = get().sessions[sessionId];
    if (!session) return;
    let item = session.projection.queue.items.find((entry) => entry.id === id);
    try {
      item = (await api.sessionQueue(session.handle, { op: "remove", id })).removed ?? item;
    } catch (error) {
      // An agent that exited still lists what was waiting; the text is
      // here, so it comes back to the composer all the same.
      if (session.projection.process !== "exited") {
        set({ error: asError(error) });
        return;
      }
      const queue = { ...session.projection.queue, items: session.projection.queue.items.filter((entry) => entry.id !== id) };
      const current = get().sessions[sessionId];
      if (current) set({ sessions: { ...get().sessions, [sessionId]: { ...current, projection: { ...current.projection, queue } } } });
    }
    if (!item) return;
    const draft = get().drafts[sessionId] ?? "";
    set({ drafts: { ...get().drafts, [sessionId]: draft.trim() ? `${draft}\n\n${item.text}` : item.text } });
    if (item.restore?.attachments?.length) get().addAttachments(sessionId, item.restore.attachments);
    for (const mention of item.restore?.mentions ?? []) get().addMention(sessionId, mention);
    get().focusComposer();
  },

  async cancel(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      await api.sessionCancel(session.handle);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async stop(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    const detached = [...session.projection.detached.values()].filter((s) => s === "active" || s === "cancellation_requested").length;
    const running = session.projection.foreground === "running" || session.projection.foreground === "awaiting_user" || session.projection.autonomous.size > 0;
    // Subagents die with the process and nothing resumes them; only what they
    // wrote to disk survives. The dialog says so before the user does it.
    const agents = [...session.projection.inspector.agents.values()].filter((agent) => agent.status === "working" || agent.status === "starting").length;
    if (running || detached > 0 || agents > 0) {
      const parts = [];
      if (running) parts.push(t("a turn is still running"));
      if (agents > 0)
        parts.push(
          agents === 1
            ? t("{n} subagent is still working and will be killed with it (only what they wrote to docs/big-thing/agents/ survives)", { n: agents })
            : t("{n} subagents are still working and will be killed with it (only what they wrote to docs/big-thing/agents/ survives)", { n: agents }),
        );
      if (detached > 0) parts.push(t("{n} detached background call(s) are active", { n: detached }));
      let confirmed = false;
      try {
        confirmed = await api.confirmDialog({
          title: t("Stop this session?"),
          message: t("Stopping shuts down this agent: {reasons}. It will be asked to cancel first; remote side effects already in flight cannot be undone. The durable transcript stays on disk and can be resumed.", { reasons: parts.join(t(" and ")) }),
          okLabel: t("Stop session"),
          cancelLabel: t("Keep running"),
          warning: true,
        });
      } catch (error) {
        set({ error: asError(error) });
        return;
      }
      if (!confirmed) return;
    }
    set({ busy: t("Stopping session") });
    try {
      await api.sessionStop(session.handle);
      const { [sessionId]: _removed, ...rest } = get().sessions;
      const order = get().sessionOrder.filter((s) => s !== sessionId);
      const view: View = get().view.kind === "session" ? { kind: "workspace", workspaceId: session.workspaceId } : get().view;
      set({ sessions: rest, sessionOrder: order, view });
      void get().inspectWorkspace(session.workspaceId);
      void get().loadRecords(session.workspaceId);
    } catch (error) {
      set({ error: asError(error) });
    } finally {
      set({ busy: null });
    }
  },

  async refreshSnapshot(sessionId) {
    const session = get().sessions[sessionId];
    if (!session) return;
    try {
      const snapshot = await api.sessionSnapshot(session.handle);
      // Deltas skipped by the lagging channel are recovered from the retained
      // history rather than guessed from the snapshot alone.
      const missed = await api.sessionHistory(session.handle, session.projection.lastSequence);
      const projection = { ...session.projection, snapshotNeeded: false, foreground: snapshot.foreground, process: snapshot.process, attachment: snapshot.attachment };
      for (const event of missed) if (BigInt(event.sequence) > BigInt(projection.lastSequence)) applyEvent(projection, event);
      for (const [id, patch] of Object.entries(snapshot.toolCalls)) if (!projection.toolCalls.has(id)) projection.toolCalls.set(id, patch);
      set({ sessions: { ...get().sessions, [sessionId]: { ...session, snapshot, projection } } });
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setConfigOption(sessionId, configId, value) {
    const session = get().sessions[sessionId];
    if (!session) return;
    set({ busy: t("Switching {option}", { option: configId }), error: null });
    try {
      const result = (await api.sessionSetConfigOption(session.handle, configId, value)) as { configOptions?: unknown } | null;
      const current = get().sessions[sessionId];
      if (current && result && typeof result === "object" && Array.isArray(result.configOptions)) {
        set({ sessions: { ...get().sessions, [sessionId]: { ...current, projection: { ...current.projection, configOptions: result.configOptions } } } });
      }
      // The accepted value becomes the default for sessions started later on
      // the same provider: a model id means nothing to another one. It does
      // not change which provider new sessions start on.
      const provider = session.snapshot.provider;
      const choice: ProviderChoice = { ...get().sessionDefaults[provider] };
      if (configId === "model") {
        choice.model = value;
      } else if (configId === "effort" || configId === "reasoning_effort") {
        if (value === "default") delete choice.reasoningEffort;
        else choice.reasoningEffort = value;
      }
      const defaults: SessionDefaults = { ...get().sessionDefaults, [provider]: choice };
      set({ sessionDefaults: defaults });
      void api.prefSet("session_defaults", defaults).catch(() => undefined);
    } catch (error) {
      set({ error: asError(error) });
    } finally {
      set({ busy: null });
    }
  },

  /**
   * Runs a provider's own sign-in: a separate process, no token through the
   * desktop, and a URL surfaced for the user to click.
   */
  async startSignIn(provider, account) {
    const previous = get().signIn;
    if (previous?.state === "starting" || previous?.state === "running") return;
    if (provider === "gemini") account ??= "default";
    const starting: SignInRun = { runId: null, provider, ...(account ? { account } : {}), lines: [], urls: [], state: "starting" };
    set({ signIn: starting, error: null });
    try {
      if (provider === "gemini") {
        const terminal = await api.providerLoginTerminal(account!);
        const run = get().signIn;
        if (run === starting) set({ signIn: { ...run, runId: terminal.id, terminal, state: "running" } });
        else await api.terminalClose(terminal.id).catch(() => undefined);
        return;
      }
      const { runId } = await api.providerLoginStart(provider, (event: LoginEvent) => {
        const run = get().signIn;
        if (!run || run.provider !== provider) return;
        let next: SignInRun = run;
        switch (event.type) {
          case "started":
            next = { ...run, state: "running" };
            break;
          case "line":
            next = { ...run, lines: [...run.lines, { stream: event.stream, text: event.text }].slice(-200) };
            break;
          case "url":
            next = { ...run, urls: run.urls.includes(event.url) ? run.urls : [...run.urls, event.url] };
            break;
          case "exited":
            next = { ...run, state: event.success ? "success" : event.cancelled ? "cancelled" : event.timedOut ? "timed_out" : "failed" };
            if (event.success) {
              void get().loadProviders();
              if (ACCOUNT_PROVIDERS.includes(provider)) void get().loadAccounts(provider);
            }
            break;
        }
        set({ signIn: next });
      }, account);
      const run = get().signIn;
      if (run && run.provider === provider) set({ signIn: { ...run, runId } });
    } catch (error) {
      if (provider !== "gemini" || get().signIn === starting) set({ error: asError(error), signIn: null });
    }
  },

  setStartingDraft(draft, queued) {
    const starting = get().starting;
    if (starting) set({ starting: { ...starting, draft, queued } });
  },

  async loadAccounts(provider) {
    try {
      const list = await api.accountsList(provider);
      set({ accounts: { ...get().accounts, [provider]: list } });
    } catch {
      // A provider that is not installed has no accounts to list.
    }
  },

  async addAccount(provider, label) {
    try {
      const added = await api.accountAdd(provider, label);
      await get().loadAccounts(provider);
      await get().startSignIn(provider, added.id);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async renameAccount(provider, id, label) {
    try {
      await api.accountRename(provider, id, label);
      await get().loadAccounts(provider);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async removeAccount(provider, id) {
    try {
      const signOutProblem = await api.accountRemove(provider, id);
      if (signOutProblem) {
        const confirmed = await api.confirmDialog({
          title: t("Remove from this computer anyway?"),
          message: t("Antigravity could not sign out. {reason} Removing the local profile leaves Google access unrevoked.", { reason: signOutProblem }),
          okLabel: t("Remove locally"),
          cancelLabel: t("Keep it"),
          warning: true,
        });
        if (!confirmed) return;
        await api.accountRemove(provider, id, true);
      }
      refreshAccountModels(get, set, provider);
      await Promise.all([get().loadAccounts(provider), get().loadProviders()]);
      void get().refreshUsage(provider);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async setActiveAccount(provider, id, sessionId) {
    try {
      await api.accountSetActive(provider, id);
    } catch (error) {
      set({ error: asError(error) });
      return;
    }
    refreshAccountModels(get, set, provider);
    // The usage bars and sign-in status follow the account.
    set({ accounts: { ...get().accounts, [provider]: (get().accounts[provider] ?? []).map((account) => ({ ...account, active: account.id === id })) } });
    void get().loadProviders();
    void get().refreshUsage(provider);
    const session = sessionId ? get().sessions[sessionId] : undefined;
    const agentSessionId = session?.snapshot.agentSessionId;
    const idle = session && session.projection.foreground !== "running" && session.projection.foreground !== "cancelling" && !session.inFlightRequestId;
    if (!session || !agentSessionId || session.snapshot.provider !== provider || !idle || session.projection.process === "exited") return;
    // The conversation moves with the account: the same session, resumed
    // under it. History is shared between accounts, so nothing is lost.
    set({ busy: t("Moving to the other account") });
    try {
      await api.sessionStop(session.handle).catch(() => undefined);
      const { [session.handle.id]: _stopped, ...rest } = get().sessions;
      set({ sessions: rest, sessionOrder: get().sessionOrder.filter((key) => key !== session.handle.id) });
    } finally {
      set({ busy: null });
    }
    // Named, because a session that was pinned to an account would otherwise
    // be reopened on that one again.
    await get().openSession(session.workspaceId, { mode: "resume", session_id: agentSessionId }, provider, { accountId: id });
  },

  async finishGeminiSignIn() {
    const run = get().signIn;
    if (run?.provider !== "gemini" || !run.terminal || run.finishing) return;
    set({ signIn: { ...run, finishing: true } });
    try {
      const list = await api.accountsList("gemini");
      if (get().signIn?.runId !== run.runId || get().signIn?.state !== "running") return;
      set({ accounts: { ...get().accounts, gemini: list } });
      const auth = list.find((entry) => entry.id === run.account)?.auth;
      if (auth?.setupRequired) {
        await api.terminalClose(run.terminal.id);
        if (get().signIn?.runId !== run.runId || get().signIn?.state !== "running") return;
        set({ signIn: null });
        await get().startSignIn("gemini", run.account);
        return;
      }
      if (!auth?.loggedIn) {
        set({ error: asError(auth?.problem ?? t("Finish signing in inside the terminal first.")) });
        return;
      }
      await api.terminalClose(run.terminal.id);
      if (get().signIn?.runId !== run.runId || get().signIn?.state !== "running") return;
      const { terminal: _terminal, ...finished } = run;
      set({ signIn: { ...finished, state: "success" }, error: null });
      await get().loadProviders();
      void get().refreshUsage("gemini");
    } catch (error) { set({ error: asError(error) }); }
    finally {
      const current = get().signIn;
      if (current?.runId === run.runId && current.finishing) set({ signIn: { ...current, finishing: false } });
    }
  },

  async cancelSignIn() {
    const run = get().signIn;
    if (!run) return;
    try {
      if (run.provider === "gemini") {
        set({ signIn: { ...run, state: "cancelled" } });
        if (run.terminal) {
          try { await api.terminalClose(run.terminal.id); }
          catch (error) { if (asError(error).code !== "NOT_READY") throw error; }
        }
        const current = get().signIn;
        if (current?.runId === run.runId && current.state === "cancelled") {
          const { terminal: _terminal, ...cancelled } = current;
          set({ signIn: cancelled });
        }
      } else if (run.runId) {
        await api.providerLoginCancel(run.runId);
      }
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  async openUrl(url) {
    try {
      await api.openExternal(url);
    } catch (error) {
      set({ error: asError(error) });
    }
  },

  focusComposer() {
    set({ focusComposerToken: get().focusComposerToken + 1 });
  },

  clearError() {
    set({ error: null });
  },
}));

/** Whether a remembered place can still be shown. */
export function viewExists(state: Pick<State, "sessions" | "workspaces">, view: View): boolean {
  if (view.kind === "session") return !!state.sessions[view.sessionId];
  if (view.kind === "workspace") return state.workspaces.some((w) => w.id === view.workspaceId);
  return true;
}

/**
 * The phone back where it was before its screens started over: the project
 * at once, then the conversation once its live session is attached again.
 * A conversation that is no longer open leaves it on its project.
 */
async function returnTo(get: Get, set: Set, place: Place, rehydrated: Promise<void>): Promise<void> {
  await get().selectWorkspace(place.workspaceId);
  if (place.kind !== "session") return;
  await rehydrated;
  const view = get().view;
  // The person moved on while the sessions were loading: leave them there.
  if (view.kind !== "workspace" || view.workspaceId !== place.workspaceId) return;
  if (!get().sessions[place.sessionId]) return;
  set({ sessionTab: place.tab, transcriptSearchOpen: false });
  get().setView({ kind: "session", sessionId: place.sessionId });
}

/** Whether the phone's start has settled where it goes, so its own moves are not remembered over the place it is returning to. */
let placeRestored = !IS_REMOTE;
useStore.subscribe((state, prev) => {
  if (!IS_REMOTE || !placeRestored || (state.view === prev.view && state.sessionTab === prev.sessionTab)) return;
  const view = state.view;
  if (view.kind === "session") {
    const workspaceId = state.sessions[view.sessionId]?.workspaceId;
    if (workspaceId) rememberPlace({ kind: "session", sessionId: view.sessionId, workspaceId, tab: state.sessionTab });
  } else if (view.kind === "workspace") {
    rememberPlace({ kind: "workspace", workspaceId: view.workspaceId });
  }
});

// Every move is recorded after the fact, once per tick: switching session sets
// the view and the panel separately, and that is one visit, not two.
let navPending = false;
useStore.subscribe((state, prev) => {
  if (navPending || (state.view === prev.view && state.sessionTab === prev.sessionTab)) return;
  navPending = true;
  queueMicrotask(() => {
    navPending = false;
    const now = useStore.getState();
    const next = recordVisit(now.navHistory, now.navIndex, navEntry(now.view, now.sessionTab));
    if (next) useStore.setState({ navHistory: next.history, navIndex: next.index });
  });
});
