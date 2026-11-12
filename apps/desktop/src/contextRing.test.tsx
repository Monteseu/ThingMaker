/**
 * The context ring: how full the window is, and a breakdown that shows only
 * what the agent's own figures add up to.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UsageCall } from "@thingmaker/contracts";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { useStore } from "./store";
import { ContextRing, compactTokens, splitLastCall } from "./components/ContextRing";

const call = (inputTokens: number, outputTokens: number): UsageCall => ({ generation: 0, itemId: "m", kind: "assistant", inputTokens, outputTokens }) as UsageCall;

function session(provider: string, used: number | null, size: number | null) {
  useStore.setState({
    sessions: { s1: { workspaceId: "w1", snapshot: { provider, agentSessionId: "agent-1" }, projection: { usage: { used, size } } } },
  } as never);
}

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
});
afterEach(cleanup);

describe("token figures", () => {
  it("read like Claude Code's own", () => {
    expect(compactTokens(958)).toBe("958");
    expect(compactTokens(33_000)).toBe("33k");
    expect(compactTokens(224_100)).toBe("224.1k");
    expect(compactTokens(1_000_000)).toBe("1M");
  });
});

describe("the breakdown", () => {
  it("splits the last request into what it sent and the reply when they add up", () => {
    expect(splitLastCall([call(5000, 100), call(220_000, 4100)], 224_100)).toEqual([
      { key: "sent", tokens: 220_000 },
      { key: "reply", tokens: 4100 },
    ]);
  });

  it("is left out when the last call does not match the agent's figure", () => {
    expect(splitLastCall([call(220_000, 4100), call(3000, 200)], 224_100)).toBeNull();
    expect(splitLastCall([], 224_100)).toBeNull();
  });
});

describe("the ring", () => {
  it("shows how full the window is and itemizes it when opened", async () => {
    session("claude", 224_100, 1_000_000);
    vi.mocked(calls.api.sessionTokenUsage = vi.fn()).mockResolvedValue({ calls: [call(220_000, 4100)] });
    render(<ContextRing sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: "Context: 22% used" }));
    expect(screen.getByText("22% used")).toBeTruthy();
    expect(screen.getByText("224.1k / 1M")).toBeTruthy();
    expect(screen.getByText("Free space")).toBeTruthy();
    await waitFor(() => expect(screen.getByText("Instructions, tools and conversation")).toBeTruthy());
    expect(screen.getByText("Last reply")).toBeTruthy();
    expect(calls.api.sessionTokenUsage).toHaveBeenCalledWith("w1", "agent-1");
  });

  it("shows only used and free space when the split is not certain", async () => {
    session("claude", 224_100, 1_000_000);
    vi.mocked(calls.api.sessionTokenUsage = vi.fn()).mockRejectedValue(new Error("no transcript"));
    render(<ContextRing sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: "Context: 22% used" }));
    await waitFor(() => expect(calls.api.sessionTokenUsage).toHaveBeenCalled());
    expect(screen.getByText("Used")).toBeTruthy();
    expect(screen.queryByText("Last reply")).toBeNull();
  });

  it("is hidden without a window size, and for Local Models", () => {
    session("gemini", 5000, null);
    const { container, rerender } = render(<ContextRing sessionId="s1" />);
    expect(container.innerHTML).toBe("");
    session("local", 5000, 16384);
    rerender(<ContextRing sessionId="s1" />);
    expect(container.innerHTML).toBe("");
  });
});
