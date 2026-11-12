/**
 * The desktop's activity: the Office has the detail, so above the prompt box
 * there are only the robots of whoever is at work, with no words.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JobView, Snapshot } from "@thingmaker/contracts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { EMPTY_SESSION_USAGE, useStore } from "./store";
import { emptyProjection } from "./projection";
import { ActivityBar } from "./components/ActivityBar";

function seed(foreground: "idle" | "running", silentMs = 1_000) {
  const handle = { id: "lead", attachmentGeneration: "1" };
  const projection = emptyProjection();
  projection.foreground = foreground;
  useStore.setState({
    sessions: { lead: {
      handle, workspaceId: "w1", snapshot: { handle, provider: "claude" } as Snapshot,
      projection, inFlightRequestId: null, steerInFlight: false, attention: "none",
      openedAt: 0, turnStartedAt: foreground === "running" ? Date.now() - silentMs : null, lastEventAt: Date.now() - silentMs, usage: EMPTY_SESSION_USAGE,
    } },
  });
}

const job = (id: string, worker: string, status: JobView["status"]): JobView => ({ id, orchestrator: "lead", worker, provider: "codex", task: "t", status, toolCalls: 0, attempts: 0, startedAtUnixMs: Date.now() - 60_000 });

beforeEach(() => {
  localStorage.clear();
  useStore.setState({
    jobs: {},
    teams: { lead: { workers: [{ name: "Luna Artist", provider: "codex", capabilities: ["image"] }, { name: "Quick Sonnet", provider: "claude", capabilities: ["fast"] }], nativeSubagents: false } },
  });
});
afterEach(cleanup);

describe("the desktop's activity", () => {
  it("shows only the robots at work, smaller, with no words", () => {
    seed("running");
    useStore.setState({ jobs: { lead: [job("j1", "Luna Artist", "running")] } });
    const { container } = render(<ActivityBar onOpenAgents={() => undefined} sessionId="lead" />);
    const robots = container.querySelectorAll(".crew-member");
    expect([...robots].map((robot) => robot.getAttribute("title")?.split(" · ")[0])).toEqual(["Lead", "Luna Artist"]);
    // Quick Sonnet is ready, not working, so it does not stand there.
    expect(screen.queryByText(/Quick Sonnet|Lead|Agents|steps? so far/)).toBeNull();
    expect(container.textContent).toBe("");
    const sizes = [...container.querySelectorAll("svg")].map((svg) => Number(svg.getAttribute("height")));
    expect(sizes).toEqual([27, 22]);
  });

  it("opens the Agents tab from the robots", () => {
    seed("running");
    const onOpenAgents = vi.fn();
    render(<ActivityBar onOpenAgents={onOpenAgents} sessionId="lead" />);
    fireEvent.click(screen.getByRole("button", { name: "Who is working" }));
    expect(onOpenAgents).toHaveBeenCalled();
  });

  it("is empty when nobody is working, and ignores the phone's preference", () => {
    localStorage.setItem("thingmaker.phone.activityCollapsed", "true");
    seed("running");
    const view = render(<ActivityBar onOpenAgents={() => undefined} sessionId="lead" />);
    expect(view.container.querySelectorAll(".crew-member")).toHaveLength(1);
    view.unmount();
    seed("idle");
    useStore.setState({ jobs: { lead: [job("j1", "Luna Artist", "succeeded")] } });
    const idle = render(<ActivityBar onOpenAgents={() => undefined} sessionId="lead" />);
    expect(idle.container.innerHTML).toBe("");
  });
});
