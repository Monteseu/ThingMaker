/**
 * Ultra-Review in the renderer: the panel it starts with, what it costs, how
 * the debate reads, and the three screens (set up, agora, verdict), with
 * reviewers named by letter until the verdict.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderModel, TeamPreset, UltraReview, UrFinding } from "@thingmaker/contracts";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { api } from "./ipc";
import { useStore } from "./store";
import { DEFAULT_CONFIG, defaultPanel, estimateTurns, fixRequest, isPrLookup, matchesPr, panelFromPreset, phaseSteps, presetFromPanel, reviewerName, stanceMatrix, withChair } from "./ultraReview";
import { UltraReviewPane } from "./components/UltraReview";

const model = (id: string, name: string, isDefault = false): ProviderModel => ({ id, name, needsCredits: false, efforts: ["low", "medium", "high"], defaultEffort: "medium", inputImage: false, isDefault });
const catalogs = {
  claude: [model("claude-opus-5-5", "Opus 5.5", true), model("claude-sonnet-5-5", "Sonnet 5.5")],
  codex: [model("gpt-5.6-sol", "GPT-5.6 Sol", true)],
  gemini: [model("gemini-3-pro", "Gemini 3 Pro", true)],
};

function finding(id: string, author: string, extra: Partial<UrFinding> = {}): UrFinding {
  return { id, author, title: `Problem ${id}`, severity: "medium", file: "src/a.ts", line: 4, thread: [{ by: author, kind: "claim", text: `${id} is wrong`, round: 0, at: 1 }], standing: "open", settled: false, ...extra };
}

function review(extra: Partial<UltraReview> = {}): UltraReview {
  return {
    id: "r1",
    workspaceId: "w1",
    host: "agent-1",
    target: { kind: "pr", number: 7, title: "Office desks", baseRef: "main", headRef: "office/desks", baseSha: "a", headSha: "b", files: 3, additions: 142, deletions: 38 },
    config: DEFAULT_CONFIG,
    reviewers: [
      { id: "A", provider: "claude", model: "claude-opus-5-5", lens: "architecture", chair: true, absent: false },
      { id: "B", provider: "codex", model: "gpt-5.6-sol", lens: "correctness", chair: false, absent: false },
      { id: "C", provider: "gemini", lens: "tests", chair: false, absent: false },
    ],
    findings: [],
    phase: "cross",
    round: 1,
    status: "running",
    root: "/tmp/checkout",
    log: [],
    createdAt: 1,
    updatedAt: 2,
    ...extra,
  };
}

describe("the panel", () => {
  it("starts with a Chair on architecture and one lens per provider", () => {
    const panel = defaultPanel(catalogs, ["claude", "codex", "gemini"]);
    expect(panel.map((seat) => [seat.provider, seat.model, seat.lens, seat.chair])).toEqual([
      ["claude", "claude-opus-5-5", "architecture", true],
      ["codex", "gpt-5.6-sol", "correctness", false],
      ["gemini", "gemini-3-pro", "tests", false],
      ["claude", "claude-sonnet-5-5", "devils_advocate", false],
    ]);
    expect(panel[0]!.effort).toBe("high");
  });

  it("makes do with the providers there are", () => {
    expect(defaultPanel(catalogs, ["claude", "codex"]).map((seat) => seat.lens)).toEqual(["architecture", "correctness", "devils_advocate"]);
    expect(defaultPanel(catalogs, ["claude"]).every((seat) => seat.provider === "claude")).toBe(true);
    expect(defaultPanel(catalogs, [])).toEqual([]);
  });

  it("takes a saved team: its lead chairs", () => {
    const preset: TeamPreset = {
      id: "p",
      name: "Studio",
      orchestrator: { provider: "codex", model: "gpt-5.6-sol" },
      combo: { workers: [{ name: "rev", provider: "claude", capabilities: ["review"], note: "Strict" }], nativeSubagents: false },
      updatedAt: 0,
    };
    const panel = panelFromPreset(preset);
    expect(panel[0]).toMatchObject({ provider: "codex", chair: true, lens: "architecture" });
    expect(panel[1]).toMatchObject({ provider: "claude", lens: "correctness", focus: "Strict", chair: false });
    expect(withChair(panel, 1).map((seat) => seat.chair)).toEqual([false, true]);
  });

  it("saves a panel as a team and loads it back with its lenses", () => {
    const seats = [
      { provider: "claude", model: "claude-opus-5-5", effort: "high", lens: "architecture", chair: true },
      { provider: "codex", lens: "correctness", chair: false, focus: "The cache" },
      { provider: "gemini", lens: "devils_advocate", chair: false },
      { provider: "claude", lens: "devils_advocate", chair: false },
    ] as const;
    const preset = presetFromPanel([...seats], "Reviewers", ["Reviewers"]);
    expect(preset.name).toBe("Reviewers-2");
    expect(preset.orchestrator).toEqual({ provider: "claude", model: "claude-opus-5-5", effort: "high" });
    expect(preset.combo.workers.map((worker) => worker.name)).toEqual(["correctness", "devils-advocate", "devils-advocate-2"]);
    expect(preset.combo.workers[0]).toMatchObject({ capabilities: ["review"], prompt: "The cache" });
    expect(panelFromPreset(preset)).toEqual(seats);
  });

  it("finds pull requests by number, link or words", () => {
    const pr = { number: 1560, title: "fix(black-market): forward offer group", headRefName: "fix/black-market", baseRefName: "train/2026/11", url: "", additions: 1, deletions: 1, changedFiles: 1, isDraft: false, author: { login: "xavwar" } };
    expect(matchesPr(pr, "#156")).toBe(true);
    expect(matchesPr(pr, "1561")).toBe(false);
    expect(matchesPr(pr, "black offer")).toBe(true);
    expect(matchesPr(pr, "xavwar train")).toBe(true);
    expect(matchesPr(pr, "trophy")).toBe(false);
    expect(isPrLookup("#12")).toBe(true);
    expect(isPrLookup("https://github.com/o/r/pull/12/files")).toBe(true);
    expect(isPrLookup("black market")).toBe(false);
  });

  it("estimates the turns a review takes", () => {
    expect(estimateTurns(4, DEFAULT_CONFIG)).toBe(4 + 2 * (4 + 2) + 1);
    expect(estimateTurns(4, { ...DEFAULT_CONFIG, quick: true })).toBe(4 + 4 + 1);
    expect(estimateTurns(0, DEFAULT_CONFIG)).toBe(0);
  });
});

describe("the debate", () => {
  it("knows each reviewer's latest stance, and who raised what", () => {
    const one = finding("F1", "B", {
      thread: [
        { by: "B", kind: "claim", text: "x", round: 0, at: 1 },
        { by: "A", kind: "challenge", text: "no", round: 1, at: 2 },
        { by: "A", kind: "confirm", text: "fair", round: 2, at: 3 },
        { by: "C", kind: "refine", text: "lower", round: 1, at: 2 },
      ],
    });
    expect(stanceMatrix(review({ findings: [one] }))[0]!.stances).toEqual({ A: "confirm", B: "author", C: "refine" });
  });

  it("names reviewers by letter until the verdict", () => {
    const running = review();
    expect(reviewerName(running, running.reviewers[0]!, catalogs)).toBe("Reviewer A");
    const done = review({ status: "done" });
    expect(reviewerName(done, done.reviewers[0]!, catalogs)).toBe("Opus 5.5");
  });

  it("marks the phases, and quick mode's skipped rebuttal", () => {
    expect(phaseSteps(review()).map((step) => step.state)).toEqual(["done", "now", "next", "next"]);
    expect(phaseSteps(review({ config: { ...DEFAULT_CONFIG, quick: true } }))[2]!.state).toBe("skipped");
    expect(phaseSteps(review({ phase: "done", status: "done" })).every((step) => step.state === "done")).toBe(true);
  });

  it("hands the confirmed findings, and the splits you chose to fix, to the session", () => {
    const done = review({
      status: "done",
      phase: "done",
      findings: [finding("F1", "A"), finding("F2", "B"), finding("F3", "C")],
      verdict: {
        recommendation: "approve_with_fixes",
        headline: "Two fixes",
        summary: "",
        byChair: true,
        items: [
          { finding: "F1", outcome: "confirmed", severity: "high", summary: "Real.", fix: "Guard it." },
          { finding: "F2", outcome: "split", summary: "Disputed.", decision: "fix" },
          { finding: "F3", outcome: "refuted", summary: "No." },
        ],
      },
    });
    const text = fixRequest(done)!;
    expect(text).toContain("pull request #7");
    expect(text).toContain("[high] Problem F1 (src/a.ts:4): Real.\n  Suggested fix: Guard it.");
    expect(text).toContain("Problem F2");
    expect(text).not.toContain("Problem F3");
    expect(fixRequest(review())).toBeNull();
  });
});

describe("the Ultra-Review tab", () => {
  beforeEach(() => {
    for (const key of Object.keys(calls.api)) delete calls.api[key];
    useStore.setState({
      sessions: { s1: { workspaceId: "w1", snapshot: { agentSessionId: "agent-1", provider: "claude" } } },
      ultraReviews: {},
      jobs: {},
      providers: [
        { provider: "claude", resolved: {}, auth: { loggedIn: true } },
        { provider: "codex", resolved: {}, auth: { loggedIn: true } },
        { provider: "gemini", resolved: {}, auth: { loggedIn: true } },
      ],
      providerModels: catalogs,
      teamPresets: [],
      loadTeamPresets: vi.fn(async () => undefined),
      loadProviderModels: vi.fn(async () => undefined),
      busy: null,
    } as never);
    localStorage.clear();
  });
  afterEach(cleanup);

  it("sets up a review of an open pull request with the default panel", async () => {
    vi.mocked(api.ultraReviewList).mockResolvedValue([]);
    vi.mocked(api.ultraReviewPullRequests).mockResolvedValue([{ number: 7, title: "Office desks", headRefName: "office/desks", baseRefName: "main", url: "", additions: 142, deletions: 38, changedFiles: 3, isDraft: false, author: { login: "pau" } }]);
    vi.mocked(api.ultraReviewStart).mockResolvedValue(review());
    render(<UltraReviewPane sessionId="s1" />);
    expect(await screen.findByText("#7 Office desks")).toBeTruthy();
    await waitFor(() => expect(screen.getAllByRole("radio", { name: "Chair: writes the verdict" })).toHaveLength(4));
    fireEvent.click(screen.getByRole("checkbox", { name: /Quick review/ }));
    fireEvent.click(screen.getByRole("button", { name: "Start Ultra-Review" }));
    await waitFor(() => expect(api.ultraReviewStart).toHaveBeenCalled());
    const request = vi.mocked(api.ultraReviewStart).mock.calls[0]![0];
    expect(request).toMatchObject({ workspaceId: "w1", host: "agent-1", kind: "pr", number: 7, config: { quick: true } });
    expect(request.seats.filter((seat) => seat.chair)).toHaveLength(1);
    expect(await screen.findByText("Findings")).toBeTruthy();
  });

  it("finds a pull request from a pasted link and reviews it", async () => {
    const listed = { number: 7, title: "Office desks", headRefName: "office/desks", baseRefName: "main", url: "", additions: 1, deletions: 1, changedFiles: 1, isDraft: false };
    const older = { ...listed, number: 1202, title: "Old fix", state: "MERGED" };
    vi.mocked(api.ultraReviewList).mockResolvedValue([]);
    vi.mocked(api.ultraReviewPullRequests).mockImplementation(async (_workspace, query = "") => (query ? [older] : [listed]));
    vi.mocked(api.ultraReviewStart).mockResolvedValue(review());
    render(<UltraReviewPane sessionId="s1" />);
    expect(await screen.findByText("#7 Office desks")).toBeTruthy();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search pull requests" }), { target: { value: "https://github.com/o/r/pull/1202" } });
    expect(await screen.findByText("#1202 Old fix")).toBeTruthy();
    expect(screen.getByText("merged")).toBeTruthy();
    await waitFor(() => expect((screen.getByRole("radio", { name: /#1202 Old fix/ }) as HTMLInputElement).checked).toBe(true));
    expect(api.ultraReviewPullRequests).toHaveBeenLastCalledWith("w1", "https://github.com/o/r/pull/1202");
    await waitFor(() => expect(screen.getAllByRole("radio", { name: "Chair: writes the verdict" })).toHaveLength(4));
    fireEvent.click(screen.getByRole("button", { name: "Start Ultra-Review" }));
    await waitFor(() => expect(vi.mocked(api.ultraReviewStart).mock.calls[0]![0]).toMatchObject({ kind: "pr", number: 1202 }));
  });

  it("saves the panel as a team under the name typed", async () => {
    const saveTeamPreset = vi.fn(async () => undefined);
    useStore.setState({ saveTeamPreset } as never);
    vi.mocked(api.ultraReviewList).mockResolvedValue([]);
    vi.mocked(api.ultraReviewPullRequests).mockResolvedValue([]);
    render(<UltraReviewPane sessionId="s1" />);
    await waitFor(() => expect(screen.getAllByRole("radio", { name: "Chair: writes the verdict" })).toHaveLength(4));
    fireEvent.click(screen.getByRole("button", { name: "Save as team…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Team name" }), { target: { value: "Night shift" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveTeamPreset).toHaveBeenCalled());
    const preset = (saveTeamPreset.mock.calls[0] as unknown as [{ name: string; combo: { workers: unknown[] } }])[0];
    expect(preset.name).toBe("Night shift");
    expect(preset.combo.workers).toHaveLength(3);
  });

  it("says so when pull requests cannot be listed, and still offers the branch", async () => {
    vi.mocked(api.ultraReviewList).mockResolvedValue([]);
    vi.mocked(api.ultraReviewPullRequests).mockRejectedValue({ code: "NOT_READY", message: "GitHub CLI (gh) is not installed" });
    render(<UltraReviewPane sessionId="s1" />);
    expect(await screen.findByText(/GitHub CLI \(gh\) is not installed/)).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: "This branch's changes" }));
    await waitFor(() => expect((screen.getByRole("button", { name: "Start Ultra-Review" }) as HTMLButtonElement).disabled).toBe(false));
  });

  it("shows the agora with authors hidden, and steps in on a finding", async () => {
    const running = review({
      findings: [finding("F1", "B", { standing: "contested", thread: [{ by: "B", kind: "claim", text: "Timer leaks.", evidence: "a.ts:4", round: 0, at: 1 }, { by: "C", kind: "challenge", text: "Cleared on unmount.", round: 1, at: 2 }] })],
    });
    vi.mocked(api.ultraReviewList).mockResolvedValue([running]);
    vi.mocked(api.ultraReviewNote).mockResolvedValue(running);
    render(<UltraReviewPane sessionId="s1" />);
    expect(await screen.findByText("Cleared on unmount.")).toBeTruthy();
    expect(screen.getAllByText("Reviewer C").length).toBeGreaterThan(0);
    expect(screen.queryByText("GPT-5.6 Sol")).toBeNull();
    // You can see which model holds each letter (the rail); the reviewers cannot.
    expect(screen.queryByText(/Opus 5.5 ·/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Step in" }));
    fireEvent.change(screen.getByLabelText("Your note on F1"), { target: { value: "Only runs in tests." } });
    fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    await waitFor(() => expect(api.ultraReviewNote).toHaveBeenCalledWith("r1", "F1", "Only runs in tests."));
  });

  it("reads the verdict, settles a split, and posts only after confirming", async () => {
    const done = review({
      status: "done",
      phase: "done",
      findings: [finding("F1", "A"), finding("F2", "B")],
      verdict: {
        recommendation: "approve_with_fixes",
        headline: "One fix, one call",
        summary: "Good change.",
        byChair: true,
        items: [
          { finding: "F1", outcome: "confirmed", severity: "medium", summary: "Real.", fix: "Guard it." },
          { finding: "F2", outcome: "split", summary: "Disputed.", for: "Profiled.", against: "Too cheap." },
        ],
      },
    });
    vi.mocked(api.ultraReviewList).mockResolvedValue([done]);
    vi.mocked(api.ultraReviewDecide).mockResolvedValue(done);
    vi.mocked(api.ultraReviewTrackRecord).mockResolvedValue([]);
    vi.mocked(api.confirmDialog).mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    render(<UltraReviewPane sessionId="s1" />);
    expect(await screen.findByText("One fix, one call")).toBeTruthy();
    expect(screen.getByText("Profiled.")).toBeTruthy();
    // The panel is named once the debate is over.
    expect(screen.getAllByText(/GPT-5.6 Sol/).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "Fix it" }));
    await waitFor(() => expect(api.ultraReviewDecide).toHaveBeenCalledWith("r1", "F2", "fix"));
    fireEvent.click(screen.getByRole("button", { name: "Post review to #7" }));
    await waitFor(() => expect(api.confirmDialog).toHaveBeenCalledTimes(1));
    expect(api.ultraReviewPost).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Post review to #7" }));
    await waitFor(() => expect(api.ultraReviewPost).toHaveBeenCalledWith("r1"));
    expect(await screen.findByRole("button", { name: "Posted to #7" })).toBeTruthy();
  });
});
