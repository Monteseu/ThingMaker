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

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({
    starting: null,
    sessions: {},
    sessionOrder: [],
    drafts: {},
    records: {},
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
  });

  it("says so when the agent could not start, keeping what was written", async () => {
    vi.mocked(api.sessionOpen).mockRejectedValue({ code: "NOT_READY", message: "Claude Code is not signed in" });
    await useStore.getState().openSession("w1", { mode: "new" }, "claude");
    render(<StartingPanel />);
    expect(screen.getByText("Claude Code could not start: Claude Code is not signed in")).toBeTruthy();
  });
});
