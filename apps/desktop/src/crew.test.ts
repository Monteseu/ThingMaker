/**
 * The crew: every run of a helper working now, or finished within the last
 * three minutes, is a robot of its own; once they are older, the helper is
 * one robot again.
 */
import { describe, expect, it } from "vitest";
import type { Combo, JobView } from "@thingmaker/contracts";
import { RECENT_RUN_MS, buildCrew, crewTicks } from "./components/crew";

const NOW = 10_000_000;
const team: Combo = { workers: [{ name: "luna", provider: "codex", capabilities: ["image"] }, { name: "sonnet", provider: "claude", capabilities: ["code"] }], nativeSubagents: false };
let next = 0;
const job = (worker: string, status: JobView["status"], extra: Partial<JobView> = {}): JobView => ({
  id: `j${(next += 1)}`,
  orchestrator: "o",
  worker,
  provider: "codex",
  task: `paint ${next}`,
  status,
  toolCalls: 0,
  attempts: 0,
  startedAtUnixMs: NOW - 60_000 + next,
  ...extra,
});
const helpers = (jobs: JobView[], now = NOW) => buildCrew({ provider: "claude", foreground: false, phase: "", jobs, team, agents: [], now }).filter((member) => member.kind === "helper");

describe("the crew", () => {
  it("is one robot per helper when nothing has run", () => {
    expect(helpers([]).map((member) => member.name)).toEqual(["luna", "sonnet"]);
  });

  it("gives every run working at once a robot of its own", () => {
    const crew = helpers([job("luna", "running"), job("luna", "running"), job("luna", "starting"), job("luna", "waiting")]);
    expect(crew.map((member) => member.name)).toEqual(["luna", "luna 2", "luna 3", "luna 4", "sonnet"]);
    expect(new Set(crew.map((member) => member.key)).size).toBe(5);
    expect(crew[0]?.key).toBe("job:luna");
  });

  it("keeps a follow-up job on its run's robot", () => {
    const first = job("luna", "succeeded", { workerSession: "w1", finishedAtUnixMs: NOW - 1000 });
    const followUp = job("luna", "running", { workerSession: "w1", continues: first.id });
    const crew = helpers([first, followUp]);
    expect(crew.filter((member) => member.name.startsWith("luna"))).toHaveLength(1);
    expect(crew[0]?.state).toBe("working");
  });

  it("keeps a finished run's robot for three minutes, then folds it back into one", () => {
    const jobs = [job("luna", "succeeded", { finishedAtUnixMs: NOW - 1000 }), job("luna", "failed", { finishedAtUnixMs: NOW - 2000 })];
    expect(helpers(jobs).filter((member) => member.name.startsWith("luna"))).toHaveLength(2);
    const later = NOW + RECENT_RUN_MS;
    const folded = helpers(jobs, later).filter((member) => member.name.startsWith("luna"));
    expect(folded).toHaveLength(1);
    expect(folded[0]).toMatchObject({ key: "job:luna", name: "luna", state: "done" });
  });

  it("keeps the clock going while a run is recent, and stops it after", () => {
    const jobs = [job("luna", "succeeded", { finishedAtUnixMs: NOW })];
    expect(crewTicks(jobs, NOW + 1000)).toBe(true);
    expect(crewTicks(jobs, NOW + RECENT_RUN_MS)).toBe(false);
    expect(crewTicks([job("luna", "running")], NOW + RECENT_RUN_MS * 10)).toBe(true);
  });
});
