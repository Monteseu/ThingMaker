/**
 * A project's home: one box starts a session with what was typed, a team
 * can be chosen first, and the recent sessions open from their row.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { useStore } from "./store";
import { WorkspaceView, homePath } from "./components/WorkspaceView";

const record = (n: number) => ({
  id: `r${n}`,
  workspaceId: "w1",
  agentSessionId: `agent-${n}`,
  autoTitle: `Task ${n}`,
  origin: "desktop",
  archiveState: "active",
  pinned: false,
  lastSeen: 1_000_000 - n,
  provider: "claude",
  createdAt: 0,
});

const preset = { id: "p1", name: "Big Brain", orchestrator: { provider: "claude", model: "opus" }, combo: { workers: [] }, updatedAt: 0 };

// Tests below swap these for mocks; each test starts from the real ones.
const { openSession: realOpenSession, openSessionWithPreset: realOpenSessionWithPreset } = useStore.getState();

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({
    workspaces: [{ id: "w1", displayPath: "/Users/someone/hunted", canonicalRoot: "/Users/someone/hunted" }],
    inspections: {
      w1: {
        identity: { canonicalRoot: "/Users/someone/hunted", workspaceHash: "w-b85f2f60bb294c35aa" },
        record: { trustState: "trusted_local" },
        sources: [
          { path: "CLAUDE.md", present: false, bytes: 0, executable: false, summary: [] },
          { path: ".mcp.json", present: false, bytes: 0, executable: false, summary: [] },
        ],
        trustDigest: "af1349b9f5f9a1a6ff",
        trustStale: false,
        isGitRepository: true,
        isUnityProject: false,
      },
    },
    records: { w1: [...Array.from({ length: 10 }, (_, n) => record(n)), { ...record(99), archiveState: "archived" }] },
    repoInfo: { w1: { head: "main", detached: false, ahead: 0, behind: 0 } },
    providers: [{ provider: "claude", resolved: {}, auth: { loggedIn: true } }],
    teamPresets: [preset],
    sessions: {},
    sessionDefaults: {},
    providerModels: {},
    starting: null,
    openSession: realOpenSession,
    openSessionWithPreset: realOpenSessionWithPreset,
  } as never);
});
afterEach(cleanup);

describe("a project's home", () => {
  it("starts a session with what was typed", () => {
    const openSession = vi.fn(async () => undefined);
    useStore.setState({ openSession } as never);
    render(<WorkspaceView workspaceId="w1" />);
    const input = screen.getByRole("textbox", { name: "Prompt" });
    fireEvent.change(input, { target: { value: "Add the reward ladder" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(openSession).toHaveBeenCalledWith("w1", { mode: "new" }, "claude", { message: "Add the reward ladder" });
  });

  it("starts with the chosen team", () => {
    const openSessionWithPreset = vi.fn(async () => undefined);
    useStore.setState({ openSessionWithPreset } as never);
    render(<WorkspaceView workspaceId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "Set team" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Big Brain" }));
    fireEvent.click(screen.getByRole("button", { name: "Start session" }));
    expect(openSessionWithPreset).toHaveBeenCalledWith("w1", preset, undefined);
  });

  it("lists recent sessions, folds the rest, and opens one from its row", () => {
    const openSession = vi.fn(async () => undefined);
    useStore.setState({ openSession } as never);
    render(<WorkspaceView workspaceId="w1" />);
    expect(screen.getByText("~/hunted")).toBeTruthy();
    expect(screen.getByText("No instructions or MCP servers in this project")).toBeTruthy();
    expect(screen.queryByText("Task 8")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show all 10" }));
    expect(screen.getByText("Task 9")).toBeTruthy();
    expect(screen.queryByText("Task 99")).toBeNull();
    fireEvent.click(screen.getByText("Task 0"));
    expect(openSession).toHaveBeenCalledWith("w1", { mode: "resume", session_id: "agent-0" });
  });
});

describe("a new session's first message", () => {
  it("is held until the agent is ready", () => {
    vi.mocked(calls.api.sessionOpen ?? (calls.api.sessionOpen = vi.fn())).mockReturnValue(new Promise(() => undefined) as never);
    void useStore.getState().openSession("w1", { mode: "new" }, "claude", { message: "Hello" });
    expect(useStore.getState().starting).toMatchObject({ draft: "Hello", queued: true });
  });
});

it("shows the home folder as ~", () => {
  expect(homePath("/Users/someone/hunted")).toBe("~/hunted");
  expect(homePath("C:\\Users\\someone\\hunted")).toBe("~\\hunted");
  expect(homePath("/opt/work")).toBe("/opt/work");
});
