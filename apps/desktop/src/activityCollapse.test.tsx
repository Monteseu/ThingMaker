import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Snapshot } from "@thingmaker/contracts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("./remote/mode", () => ({ IS_REMOTE: true }));

import { EMPTY_SESSION_USAGE, useStore } from "./store";
import { emptyProjection } from "./projection";
import { ActivityBar } from "./components/ActivityBar";

beforeEach(() => {
  localStorage.clear();
  const handle = { id: "lead", attachmentGeneration: "1" };
  const projection = emptyProjection();
  projection.foreground = "running";
  useStore.setState({
    sessions: { lead: {
      handle, workspaceId: "w1", snapshot: { handle, provider: "claude" } as Snapshot,
      projection, inFlightRequestId: null, steerInFlight: false, attention: "none",
      openedAt: 0, turnStartedAt: Date.now(), lastEventAt: Date.now(), usage: EMPTY_SESSION_USAGE,
    } }, jobs: {}, teams: {},
  });
});
afterEach(cleanup);

it("hides the mobile team panel, remembers the choice, and can show it again", () => {
  const draw = () => render(<ActivityBar sessionId="lead" onOpenAgents={() => undefined} />);
  const view = draw();
  expect(screen.getByRole("list", { name: "Who is working" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Hide team activity" }));
  expect(screen.queryByRole("list", { name: "Who is working" })).toBeNull();
  expect(screen.getByRole("button", { name: "Show team activity" }).getAttribute("aria-expanded")).toBe("false");
  expect(useStore.getState().sessions.lead?.projection.foreground).toBe("running");
  view.unmount();
  draw();
  expect(screen.queryByRole("list", { name: "Who is working" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Show team activity" }));
  expect(screen.getByRole("list", { name: "Who is working" })).toBeTruthy();
});
