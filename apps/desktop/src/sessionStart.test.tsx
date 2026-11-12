/**
 * A new session opens its chat at once: the person can write while the
 * agent starts, and what they send goes out as soon as it is ready.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { api } from "./ipc";
import { useStore } from "./store";
import { StartingPanel } from "./components/StartingPanel";
import { Sidebar } from "./components/Sidebar";

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({
    starting: null,
    sessions: {},
    sessionOrder: [],
    drafts: {},
    records: {},
    inspections: { w1: { trustStale: false, record: { trustState: "trusted_local" } } },
    workspaces: [{ id: "w1", displayPath: "/work/game", canonicalRoot: "/work/game" }],
    providers: [{ provider: "claude", resolved: {}, auth: { loggedIn: true } }],
    sessionDefaults: {},
    loadOdyssey: vi.fn(async () => undefined),
    loadTeam: vi.fn(async () => undefined),
    loadNesting: vi.fn(async () => undefined),
  } as never);
});
afterEach(cleanup);

describe("a new session", () => {
  it("shows its chat at once and sends what was written once it is ready", async () => {
    let ready: (value: unknown) => void = () => undefined;
    vi.mocked(api.sessionOpen).mockReturnValue(new Promise((resolve) => (ready = resolve)) as never);
    vi.mocked(api.sessionSnapshot).mockResolvedValue({ handle: { id: "s1", attachmentGeneration: "1" }, provider: "claude", agentSessionId: "agent-1", toolCalls: {}, foreground: "idle", history: [] } as never);
    vi.mocked(api.sessionHistory).mockResolvedValue([] as never);
    const send = vi.fn(async () => undefined);
    useStore.setState({ send } as never);

    const opening = useStore.getState().openSession("w1", { mode: "new" }, "claude");
    expect(useStore.getState().view).toEqual({ kind: "starting" });
    expect(useStore.getState().busy).toBeNull();
    render(<StartingPanel />);
    expect(screen.getByRole("status").textContent).toContain("Setting up the office…");
    expect(screen.getByText("Starting Claude Code…")).toBeTruthy();
    const input = screen.getByRole("textbox", { name: "Prompt" });
    fireEvent.change(input, { target: { value: "Fix the login bug" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("Sends as soon as Claude Code is ready")).toBeTruthy();
    expect(send).not.toHaveBeenCalled();

    ready({ handle: { id: "s1", attachmentGeneration: "1" }, snapshot: {} });
    await opening;
    await waitFor(() => expect(send).toHaveBeenCalledWith("s1"));
    expect(useStore.getState().drafts.s1).toBe("Fix the login bug");
    expect(useStore.getState().starting).toBeNull();
    expect(useStore.getState().view).toEqual({ kind: "session", sessionId: "s1" });
  });

  it("opens from the project + immediately and keeps feedback until history is ready", async () => {
    let opened: (value: unknown) => void = () => undefined;
    let historyReady: (value: unknown) => void = () => undefined;
    vi.mocked(api.sessionOpen).mockReturnValue(new Promise((resolve) => (opened = resolve)) as never);
    vi.mocked(api.sessionSnapshot).mockResolvedValue({ handle: { id: "s1", attachmentGeneration: "1" }, provider: "claude", toolCalls: {}, foreground: "idle" } as never);
    vi.mocked(api.sessionHistory).mockReturnValue(new Promise((resolve) => (historyReady = resolve)) as never);
    function View() {
      const view = useStore((s) => s.view);
      return <><Sidebar />{view.kind === "starting" && <StartingPanel />}</>;
    }
    useStore.setState({ view: { kind: "workspace", workspaceId: "w1" } });
    render(<View />);
    fireEvent.click(screen.getByRole("button", { name: "New session" }));
    expect(screen.getByRole("status").textContent).toContain("Setting up the office…");
    fireEvent.change(screen.getByRole("textbox", { name: "Prompt" }), { target: { value: "My draft" } });
    opened({ handle: { id: "s1", attachmentGeneration: "1" } });
    await waitFor(() => expect(api.sessionHistory).toHaveBeenCalled());
    expect(useStore.getState().view).toEqual({ kind: "starting" });
    expect(screen.getByRole("textbox", { name: "Prompt" }).getAttribute("disabled")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("Setting up the office…");
    historyReady([]);
    await waitFor(() => expect(useStore.getState().view).toEqual({ kind: "session", sessionId: "s1" }));
    expect(useStore.getState().drafts.s1).toBe("My draft");
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("does not take you back to the chat if you navigate away while it starts", async () => {
    let ready: (value: unknown) => void = () => undefined;
    vi.mocked(api.sessionOpen).mockReturnValue(new Promise((resolve) => (ready = resolve)) as never);
    vi.mocked(api.sessionSnapshot).mockResolvedValue({ provider: "claude", toolCalls: {} } as never);
    vi.mocked(api.sessionHistory).mockResolvedValue([] as never);
    const opening = useStore.getState().openSession("w1", { mode: "new" }, "claude");
    useStore.getState().setView({ kind: "settings" });
    ready({ handle: { id: "s1", attachmentGeneration: "1" } });
    await opening;
    expect(useStore.getState().view).toEqual({ kind: "settings" });
    expect(useStore.getState().sessions.s1).toBeTruthy();
  });

  it("says so when the agent could not start, keeping what was written", async () => {
    vi.mocked(api.sessionOpen).mockRejectedValue({ code: "NOT_READY", message: "Claude Code is not signed in" });
    await useStore.getState().openSession("w1", { mode: "new" }, "claude");
    render(<StartingPanel />);
    expect(screen.getByText("Claude Code could not start: Claude Code is not signed in")).toBeTruthy();
  });
});
