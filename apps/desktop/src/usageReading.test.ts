/** The sidebar's account usage: a session's bare "allowed" must not erase a measured reading. */
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { useStore } from "./store";

const measured = { provider: "claude" as const, status: "allowed" as const, observedAtUnixMs: 1, windows: [{ kind: "five_hour", usedPercent: 38, windowMinutes: 300 }] };
const bare = { provider: "claude" as const, status: "allowed" as const, observedAtUnixMs: 2, windows: [] };

describe("account usage readings", () => {
  it("keeps a measured reading when a session reports only that it is allowed", () => {
    useStore.setState({ usage: {} });
    useStore.getState().noteQuota(measured);
    useStore.getState().noteQuota(bare);
    expect(useStore.getState().usage.claude?.primary?.usedPercent).toBe(38);
  });

  it("clears a refusal once a turn is allowed again", () => {
    useStore.setState({ usage: {} });
    useStore.getState().noteQuota({ ...bare, status: "rejected", windows: [{ kind: "five_hour", resetsAt: 9 }] });
    expect(useStore.getState().usage.claude?.limitReached).toBe(true);
    useStore.getState().noteQuota(bare);
    expect(useStore.getState().usage.claude).toBeNull();
  });
});

it("keeps every Antigravity model bucket with its own label and reset", () => {
  useStore.setState({ usage: {} });
  useStore.getState().noteQuota({ provider: "gemini", status: "warning", observedAtUnixMs: 10, windows: [
    { kind: "gemini-five-hour", usedPercent: 20, windowMinutes: 300, resetsAt: 100 },
    { kind: "gemini-weekly", usedPercent: 40, windowMinutes: 10080, resetsAt: 200 },
    { kind: "3p-weekly", usedPercent: 100, windowMinutes: 10080, resetsAt: 300 },
  ] });
  const usage = useStore.getState().usage.gemini!;
  expect(usage.windows?.map(w => w.label)).toEqual(["Gemini · 5h", "Gemini · 7d", "Claude/GPT · 7d"]);
  expect(usage.windows?.[2]?.resetAtUnix).toBe(300);
  expect(usage.limitReached).toBe(false);
});
