/**
 * The Teams view's Export picks teams and saves them; Import adds a file's
 * teams beside the ones already here.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TeamPreset } from "@thingmaker/contracts";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { api } from "./ipc";
import { useStore } from "./store";
import { TeamsTransfer } from "./components/TeamsTransfer";
import { exportTeams } from "./teamFile";

const preset = (name: string): TeamPreset => ({ id: name, name, orchestrator: { provider: "claude" }, combo: { workers: [], nativeSubagents: false }, updatedAt: 1 });

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({ teamPresets: [preset("Art crew"), preset("Solo")], helperPresets: [] } as never);
});
afterEach(cleanup);

describe("exporting and importing teams", () => {
  it("exports the teams that are ticked", async () => {
    vi.mocked(api.saveTextFile).mockResolvedValue("/tmp/solo.team.json");
    render(<TeamsTransfer onImported={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Art crew" }));
    fireEvent.click(screen.getByRole("button", { name: "Export 1 team" }));
    await waitFor(() => expect(api.saveTextFile).toHaveBeenCalled());
    const [name, content] = vi.mocked(api.saveTextFile).mock.calls[0] as [string, string];
    expect(name).toBe("solo.team.json");
    expect(JSON.parse(content).teams.map((team: { name: string }) => team.name)).toEqual(["Solo"]);
    expect(await screen.findByText("Exported “Solo”.")).toBeTruthy();
  });

  it("imports a file's teams as new ones, and selects the first", async () => {
    const onImported = vi.fn();
    render(<TeamsTransfer onImported={onImported} />);
    const file = new File([exportTeams([preset("Solo"), preset("Reviewers")])], "teams.json", { type: "application/json" });
    fireEvent.change(screen.getByLabelText("Team file to import"), { target: { files: [file] } });
    expect(await screen.findByText(/Imported 2 teams\. Names already in use got a number\./)).toBeTruthy();
    const names = useStore.getState().teamPresets.map((entry) => entry.name);
    expect(names).toEqual(expect.arrayContaining(["Art crew", "Solo", "Solo-2", "Reviewers"]));
    expect(api.prefSet).toHaveBeenCalledWith("teamPresets", expect.any(Array));
    expect(onImported).toHaveBeenCalledWith(useStore.getState().teamPresets.find((entry) => entry.name === "Solo-2")?.id);
  });

  it("says what is wrong with a file it cannot read", async () => {
    render(<TeamsTransfer onImported={() => undefined} />);
    fireEvent.change(screen.getByLabelText("Team file to import"), { target: { files: [new File(["nope"], "x.json")] } });
    expect((await screen.findByRole("alert")).textContent).toBe("That file is not JSON.");
    expect(useStore.getState().teamPresets).toHaveLength(2);
  });

  it("exports helper presets with the teams, and imports them back as helper presets", async () => {
    const fixer = { id: "h1", name: "Artist Fixer", worker: { name: "Artist Fixer", provider: "codex" as const, capabilities: ["image", "ui"], prompt: "Flat pastel style" }, updatedAt: 1 };
    useStore.setState({ helperPresets: [fixer] } as never);
    vi.mocked(api.saveTextFile).mockResolvedValue("/tmp/thingmaker-teams.json");
    render(<TeamsTransfer onImported={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Art crew" }));
    fireEvent.click(screen.getByRole("button", { name: "Export 1 team and 1 helper" }));
    await waitFor(() => expect(api.saveTextFile).toHaveBeenCalled());
    const [, content] = vi.mocked(api.saveTextFile).mock.calls[0] as [string, string];
    expect(JSON.parse(content).helpers.map((helper: { name: string }) => helper.name)).toEqual(["Artist Fixer"]);

    useStore.setState({ helperPresets: [] } as never);
    fireEvent.change(screen.getByLabelText("Team file to import"), { target: { files: [new File([content], "t.json")] } });
    expect(await screen.findByText(/Imported 1 team and 1 helper\./)).toBeTruthy();
    expect(useStore.getState().helperPresets.map((entry) => entry.worker.prompt)).toEqual(["Flat pastel style"]);
    expect(api.prefSet).toHaveBeenCalledWith("helperPresets", expect.any(Array));
  });
});
