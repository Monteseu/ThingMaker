/**
 * The composer's team chip: a menu of the saved teams, each with who is on
 * it when pointed at, and a way to the Teams view. Picking a team sets its
 * helpers at once; a lead on another provider waits for the next message.
 */
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Combo, ProviderModel, Snapshot, TeamPreset, WorkerSlot } from "@thingmaker/contracts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {
    throw new Error("invoke is not available in tests");
  }),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { EMPTY_SESSION_USAGE, useStore, type LiveSession } from "./store";
import { emptyProjection } from "./projection";
import { TeamChip } from "./components/TeamEditor";
import { ConfigPickers } from "./components/ConfigPickers";
import { ProviderBadge } from "./components/ProviderMark";
import { presetMatches } from "./team";
import { isRetiredModel } from "@thingmaker/contracts";

const model = (id: string, name: string, efforts: string[]): ProviderModel => ({ id, name, needsCredits: false, efforts, inputImage: true, isDefault: false });
const catalogs = {
  claude: [model("opus", "Opus 5.5", ["low", "high", "xhigh"]), model("sonnet", "Sonnet 5.5", ["low", "high"])],
  codex: [model("gpt-6-luna", "GPT-6 Luna", ["low", "max"]), model("gpt-6.1-sol", "GPT-6.1 Sol", ["low", "high"])],
  gemini: [],
};
const artist: WorkerSlot = { name: "Luna Artist", provider: "codex", model: "gpt-6-luna", effort: "max", capabilities: ["image", "ui"], note: "Image generation", prompt: "Flat pastel art, no gradients" };
const reviewer: WorkerSlot = { name: "Art Reviewer", provider: "claude", model: "sonnet", effort: "high", capabilities: ["review"] };
const studio: TeamPreset = { id: "p1", name: "Art Fixer", orchestrator: { provider: "claude", model: "opus", effort: "high" }, combo: { workers: [artist, reviewer], nativeSubagents: false }, updatedAt: 1 };
const codexLed: TeamPreset = { id: "p2", name: "Codex crew", orchestrator: { provider: "codex", model: "gpt-6.1-sol" }, combo: { workers: [reviewer], nativeSubagents: false }, updatedAt: 1 };

const options = [
  { id: "model", name: "Model", type: "select", currentValue: "sonnet", options: [{ value: "opus", name: "Opus 5.5" }, { value: "sonnet", name: "Sonnet 5.5" }] },
  { id: "effort", name: "Effort", type: "select", currentValue: "low", options: [{ value: "low", name: "Low" }, { value: "high", name: "High" }] },
];

function live(): LiveSession {
  const handle = { id: "s1", attachmentGeneration: "1" };
  const snapshot = { handle, agentSessionId: "agent-s1", provider: "claude", toolCalls: {}, foreground: "idle", configOptions: options } as unknown as Snapshot;
  return { handle, workspaceId: "w1", snapshot, projection: emptyProjection(), inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
}

const saveTeam = vi.fn(async (_session: string, combo: Combo) => {
  useStore.setState({ teams: { s1: combo } });
});
const setConfigOption = vi.fn(async () => undefined);
const setView = vi.fn();

beforeEach(() => {
  saveTeam.mockClear();
  setConfigOption.mockClear();
  setView.mockClear();
  useStore.setState({
    sessions: { s1: live() },
    teams: { s1: { workers: [{ name: "Quick Sonnet", provider: "claude", model: "sonnet", capabilities: ["fast"] }], nativeSubagents: false } },
    teamPresets: [studio, codexLed],
    jobs: {},
    pendingProvider: {},
    providerModels: catalogs,
    loadProviderModels: vi.fn(async () => undefined),
    saveTeam,
    setConfigOption,
    setView,
  } as never);
});
afterEach(cleanup);

describe("team chip", () => {
  it("opens a menu of the saved teams instead of an editor", () => {
    render(<TeamChip disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /team: Quick Sonnet/ }));
    const menu = screen.getByRole("menu");
    expect(within(menu).getByRole("menuitemradio", { name: /This session's own team/ }).getAttribute("aria-checked")).toBe("true");
    expect(within(menu).getByRole("menuitemradio", { name: /Art Fixer/ }).textContent).toContain("Opus 5.5 leads · 2 helpers");
    expect(screen.queryByText("Saved team")).toBeNull();
  });

  it("shows who is on a team when it is pointed at", () => {
    render(<TeamChip disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /team: / }));
    fireEvent.mouseEnter(screen.getByRole("menuitemradio", { name: /Art Fixer/ }));
    const card = screen.getByRole("note", { name: "Who is on Art Fixer" });
    expect(card.textContent).toContain("Opus 5.5");
    expect(card.textContent).toContain("Luna Artist");
    expect(card.textContent).toContain("Artist");
    expect(card.textContent).toContain("GPT-6 Luna · max");
    expect(card.textContent).toContain("Image generation");
    expect(card.textContent).toContain("Flat pastel art");
    expect(card.textContent).toContain("Reviewer");
  });

  it("picks a team: its helpers at once, its lead's model and effort in place", async () => {
    render(<TeamChip disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /team: / }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Art Fixer/ }));
    await vi.waitFor(() => expect(setConfigOption).toHaveBeenCalledTimes(2));
    expect(saveTeam).toHaveBeenCalledWith("s1", studio.combo);
    expect(setConfigOption).toHaveBeenCalledWith("s1", "model", "opus");
    expect(setConfigOption).toHaveBeenCalledWith("s1", "effort", "high");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("button", { name: /team: Art Fixer/ })).toBeTruthy();
  });

  it("leaves a lead on another provider for the next message", async () => {
    render(<TeamChip disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /team: / }));
    fireEvent.mouseEnter(screen.getByRole("menuitemradio", { name: /Codex crew/ }));
    expect(screen.getByRole("note").textContent).toContain("Led by Codex");
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Codex crew/ }));
    await vi.waitFor(() => expect(useStore.getState().pendingProvider.s1).toEqual({ provider: "codex", model: "gpt-6.1-sol" }));
    expect(setConfigOption).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /team: Codex crew/ })).toBeTruthy();
  });

  it("goes to Teams to make or change one", () => {
    render(<TeamChip disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /team: / }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Go to Teams…" }));
    expect(setView).toHaveBeenCalledWith({ kind: "teams" });
  });
});

describe("a new session", () => {
  const solo: TeamPreset = { id: "p3", name: "Solo", orchestrator: { provider: "claude", model: "opus" }, combo: { workers: [], nativeSubagents: false }, updatedAt: 1 };

  it("has no team: the chip offers to set one, and the model stays free", () => {
    useStore.setState({ teams: {}, teamPresets: [studio, solo] } as never);
    render(<TeamChip disabled={false} sessionId="s1" />);
    expect(screen.getByRole("button", { name: /Set team/ })).toBeTruthy();
    expect(presetMatches(solo, "claude", { workers: [], nativeSubagents: false })).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: /Set team/ }));
    expect(screen.queryByText("This session's own team")).toBeNull();
    expect(within(screen.getByRole("menu")).getByText("Art Fixer")).toBeTruthy();
  });

  it("offers Ultracode inside the model menu, not as a chip", () => {
    const withUltracode = [...options, { id: "ultracode", name: "Ultracode", type: "select", currentValue: "off", options: [{ value: "off", name: "Off" }, { value: "on", name: "On" }] }];
    render(<ConfigPickers configOptions={withUltracode} disabled={false} sessionId="s1" />);
    expect(screen.queryByRole("combobox")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Sonnet 5.5/ }));
    const group = screen.getByRole("group", { name: "Ultracode" });
    fireEvent.click(within(group).getByRole("button", { name: "On" }));
    expect(setConfigOption).toHaveBeenCalledWith("s1", "ultracode", "on");
  });

  it("hides older model generations but keeps the one a session is on", () => {
    expect(isRetiredModel("claude", { id: "haiku", name: "Haiku 4.5" })).toBe(true);
    expect(isRetiredModel("claude", { id: "claude-sonnet-4-5-20250929", name: "Sonnet 4.5" })).toBe(true);
    expect(isRetiredModel("gemini", { id: "claude-opus-4-6-thinking", name: "Claude Opus 4.6 (Thinking)" })).toBe(true);
    expect(isRetiredModel("codex", { id: "gpt-5.6-sol", name: "GPT-5.6 Sol" })).toBe(true);
    for (const [provider, id, name] of [["claude", "opus", "Opus 5.5"], ["claude", "claude-fable-5-1", "Fable 5.1"], ["claude", "default", "Default (recommended)"], ["codex", "gpt-6.1-sol", "GPT-6.1 Sol"], ["gemini", "gemini-3.8-flash", "Gemini 3.8 Flash"]] as const)
      expect(isRetiredModel(provider, { id, name })).toBe(false);
    const listed = [{ id: "model", name: "Model", type: "select", currentValue: "haiku", options: [{ value: "opus", name: "Opus 5.5" }, { value: "haiku", name: "Haiku 4.5" }, { value: "claude-sonnet-4-5", name: "Sonnet 4.5" }] }];
    render(<ConfigPickers configOptions={listed} disabled={false} sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: /Haiku 4.5/ }));
    const names = within(screen.getByRole("listbox")).getAllByRole("option").map((option) => option.textContent);
    expect(names).toEqual(["Opus 5.5", "Haiku 4.5"]);
  });

  it("draws Local Models with its own mark", () => {
    const { container } = render(<ProviderBadge provider="local" />);
    expect(container.querySelector(".provider-local")).toBeTruthy();
    expect(container.querySelector("linearGradient, radialGradient")).toBeNull();
  });
});
