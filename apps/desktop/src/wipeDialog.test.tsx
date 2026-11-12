/**
 * Wipe All: counts only ThingMaker's own Claude and Codex sessions, waits for
 * open sessions to stop, and reports what it removed.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { useStore } from "./store";
import { WipeDialog } from "./components/WipeDialog";

const record = (id: string, provider: string, origin: string) => ({ id, workspaceId: "w1", agentSessionId: id, origin, archiveState: "active", pinned: false, provider, createdAt: 0 });

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({
    sessions: {},
    records: { w1: [record("a", "claude", "desktop"), record("b", "codex", "desktop"), record("c", "claude", "fork"), record("d", "claude", "cli"), record("e", "gemini", "desktop")] },
  } as never);
});
afterEach(cleanup);

describe("wiping a project's sessions", () => {
  it("counts only ThingMaker's own Claude and Codex sessions", () => {
    render(<WipeDialog name="hunted" onClose={() => undefined} workspaceId="w1" />);
    expect(screen.getByText(/the 2 Claude Code and 1 Codex sessions ThingMaker ran/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete 3 sessions permanently" })).toBeTruthy();
  });

  it("waits for open sessions to stop", () => {
    useStore.setState({ sessions: { live: { workspaceId: "w1", projection: { process: "ready" } } } } as never);
    render(<WipeDialog name="hunted" onClose={() => undefined} workspaceId="w1" />);
    expect(screen.getByText("1 session is still open. Stop it first.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Delete 3 sessions permanently" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("deletes on confirmation and says what went", async () => {
    calls.api.workspaceWipeSessions = vi.fn(async () => ({ claudeSessions: 2, codexSessions: 1, filesRemoved: 9, historyLinesRemoved: 0, codexDeletedByCodex: 1, problems: [] }));
    render(<WipeDialog name="hunted" onClose={() => undefined} workspaceId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "Delete 3 sessions permanently" }));
    await waitFor(() => expect(screen.getByText("Sessions deleted")).toBeTruthy());
    expect(calls.api.workspaceWipeSessions).toHaveBeenCalledWith("w1");
    expect(screen.getByText(/2 Claude Code and 1 Codex sessions from hunted are gone from this computer \(9 files and folders\)/)).toBeTruthy();
  });
});
