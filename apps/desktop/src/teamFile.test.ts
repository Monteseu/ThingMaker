/**
 * Teams as JSON files: what is exported reads back as the same teams, and a
 * file that is not one says what is wrong instead of importing half a team.
 */
import { describe, expect, it } from "vitest";
import type { TeamPreset } from "@thingmaker/contracts";
import { exportFileName, exportTeams, importedHelpers, importedPresets, readTeamsFile, TEAMS_FORMAT } from "./teamFile";
import type { HelperPreset } from "./team";

const preset = (name: string, id = name): TeamPreset => ({
  id,
  name,
  orchestrator: { provider: "claude", model: "opus", effort: "high" },
  combo: { workers: [{ name: "luna", provider: "codex", model: "gpt-5.6", capabilities: ["image"], note: "Images", prompt: "Pixel art only" }], nativeSubagents: true },
  updatedAt: 1,
});

describe("exporting teams", () => {
  it("round-trips restrictions, distinguishes empty from inherited, and rejects malformed policies", () => {
    const team = preset("Restricted");
    team.combo.workers[0]!.resources = { skills: ["skill:user:art"], mcps: [] };
    expect(JSON.parse(exportTeams([team])).version).toBe(2);
    const roundtrip = readTeamsFile(exportTeams([team]));
    expect(roundtrip.teams[0]?.combo.workers[0]?.resources).toEqual({ skills: ["skill:user:art"], mcps: [] });
    team.combo.workers[0]!.resources = { skills: [], mcps: [] };
    expect(readTeamsFile(exportTeams([team])).teams[0]?.combo.workers[0]?.resources).toEqual({ skills: [], mcps: [] });
    expect(readTeamsFile(exportTeams([preset("Inherited")])).teams[0]?.combo.workers[0]?.resources).toBeUndefined();
    const malformed = JSON.parse(exportTeams([team]));
    malformed.teams[0].combo.workers[0].resources = { skills: "all", mcps: [] };
    expect(() => readTeamsFile(JSON.stringify(malformed))).toThrow("Invalid member resource selection");
  });
  it("writes the teams without ids, in a file that says what it is", () => {
    const file = JSON.parse(exportTeams([preset("Art crew"), preset("Solo")], [], new Date("2026-11-06T00:00:00Z")));
    expect(file).toMatchObject({ format: TEAMS_FORMAT, version: 1, exportedAt: "2026-11-06T00:00:00.000Z" });
    expect(file.teams).toHaveLength(2);
    expect(file.teams[0]).toEqual({ name: "Art crew", orchestrator: preset("x").orchestrator, combo: preset("x").combo });
    expect(file.helpers).toBeUndefined();
  });

  it("carries saved helpers beside the teams", () => {
    const fixer: HelperPreset = { id: "h1", name: "Artist Fixer", worker: { name: "Old name", provider: "codex", model: "gpt-6.1-sol", effort: "high", capabilities: ["image", "ui"], note: "All icon work", prompt: "Flat pastel style" }, updatedAt: 1 };
    const raw = exportTeams([], [fixer]);
    expect(JSON.parse(raw).helpers).toEqual([{ ...fixer.worker, name: "Artist Fixer" }]);
    expect(exportFileName([], [fixer])).toBe("thingmaker-helpers.json");
    expect(exportFileName([preset("Solo")], [fixer])).toBe("thingmaker-teams.json");
    const read = readTeamsFile(raw);
    expect(read.teams).toEqual([]);
    expect(read.helpers).toEqual([{ ...fixer.worker, name: "Artist Fixer" }]);
    const added = importedHelpers(read.helpers, [fixer]);
    expect(added[0]).toMatchObject({ name: "Artist Fixer-2", worker: { name: "Artist Fixer-2", prompt: "Flat pastel style" } });
    expect(added[0]?.id).not.toBe("h1");
  });

  it("names the file after the team when there is one", () => {
    expect(exportFileName([preset("Luna Artist + Quick Sonnet")])).toBe("luna-artist-quick-sonnet.team.json");
    expect(exportFileName([preset("A"), preset("B")])).toBe("thingmaker-teams.json");
  });
});

describe("importing teams", () => {
  it("reads back what was exported", () => {
    const entries = readTeamsFile(exportTeams([preset("Art crew")])).teams;
    expect(entries).toEqual([{ name: "Art crew", orchestrator: preset("x").orchestrator, combo: preset("x").combo }]);
  });

  it("takes a bare list of teams, or a single team", () => {
    const team = { name: "Solo", orchestrator: { provider: "codex" }, combo: { workers: [] } };
    expect(readTeamsFile(JSON.stringify([team])).teams).toHaveLength(1);
    expect(readTeamsFile(JSON.stringify(team)).teams[0]).toEqual({ name: "Solo", orchestrator: { provider: "codex" }, combo: { workers: [], nativeSubagents: false } });
  });

  it("drops what it does not know and keeps what a team needs", () => {
    const team = { name: " Tidy ", extra: 1, orchestrator: { provider: "claude", model: "", future: true }, combo: { workers: [{ name: "a", provider: "gemini", capabilities: ["ui", 3, ""] }] } };
    expect(readTeamsFile(JSON.stringify([team])).teams[0]).toEqual({ name: "Tidy", orchestrator: { provider: "claude" }, combo: { workers: [{ name: "a", provider: "gemini", capabilities: ["ui"] }], nativeSubagents: false } });
  });

  it("says what is wrong with a file that is not one", () => {
    expect(() => readTeamsFile("not json")).toThrow("That file is not JSON.");
    expect(() => readTeamsFile(JSON.stringify({ format: "other", teams: [] }))).toThrow("not a ThingMaker teams file");
    expect(() => readTeamsFile(JSON.stringify({ format: TEAMS_FORMAT, version: 9, teams: [] }))).toThrow("newer ThingMaker");
    expect(() => readTeamsFile(JSON.stringify({ format: TEAMS_FORMAT, version: 1, teams: [] }))).toThrow("no teams or helpers");
    expect(() => readTeamsFile(JSON.stringify({ format: TEAMS_FORMAT, version: 1, teams: [], helpers: [{ name: "x", provider: "kit" }] }))).toThrow("Helper preset “x”: the helper “x” needs a provider");
    expect(() => readTeamsFile(JSON.stringify([{ name: "X", orchestrator: { provider: "kit" } }]))).toThrow("Team “X”: its lead needs a provider");
    expect(() => readTeamsFile(JSON.stringify([{ name: "X", orchestrator: { provider: "claude" }, combo: { workers: [{ name: "a", provider: "codex" }, { name: "A", provider: "claude" }] } }]))).toThrow("two helpers are called");
  });

  it("makes new teams, named apart from the ones already here and from each other", () => {
    const added = importedPresets(readTeamsFile(exportTeams([preset("Art crew"), preset("Art crew", "b")])).teams, [preset("Art crew", "old")]);
    expect(added.map((entry) => entry.name)).toEqual(["Art crew-2", "Art crew-3"]);
    expect(new Set(added.map((entry) => entry.id)).size).toBe(2);
    expect(added.some((entry) => entry.id === "old")).toBe(false);
  });
});
