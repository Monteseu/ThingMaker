/**
 * Who is on a session's crew and what each is doing: the lead, each helper
 * on the team by its latest job, and the lead's own subagents. Shared by the
 * activity strip (a row of tiny robots) and the office (the same robots at
 * their desks), so both always agree.
 */
import { t } from "../i18n";
import { closestRole } from "../teamRoles";
import type { BotRole, BotState } from "./TinyBot";
import type { JobView, Provider, Combo } from "@thingmaker/contracts";

/** One robot of the crew. */
export type CrewMember = {
  key: string;
  provider: Provider;
  role: BotRole | null;
  name: string;
  state: BotState;
  status: string;
  title: string;
  /** Where it comes from: the lead, a helper on the team, or one of the lead's own subagents. */
  kind: "lead" | "helper" | "subagent";
  /** The model it runs, when it says. */
  model: string | null;
  /** What it was asked to do, whole, for the roster. */
  task: string;
};

export function elapsed(ms: number): string {
  const s = Math.floor(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** One of the lead's own subagents, as the inspector reports it. */
export type CrewAgent = {
  id: string;
  name: string;
  task: string;
  generationStartedAtUnixMs: number | null;
  status?: string;
  outcome?: string | null;
  model?: string | null;
};

/** How long a finished run keeps its own desk before it folds back into its helper's one. */
export const RECENT_RUN_MS = 3 * 60 * 1000;

/** A run still working, waiting out a limit, or finished within {@link RECENT_RUN_MS}. */
export function isRecent(job: JobView, now: number): boolean {
  if (job.status === "starting" || job.status === "running" || job.status === "waiting") return true;
  return typeof job.finishedAtUnixMs === "number" && now - job.finishedAtUnixMs < RECENT_RUN_MS;
}

/** Whether the crew can still change by the clock alone: a run's time ticking, or a finished run about to fold away. */
export function crewTicks(jobs: JobView[] | undefined, now: number): boolean {
  return (jobs ?? []).some((job) => isRecent(job, now));
}

export function buildCrew(input: {
  provider: Provider;
  foreground: boolean;
  phase: string;
  jobs: JobView[] | undefined;
  team: Combo | undefined;
  agents: CrewAgent[];
  now: number;
}): CrewMember[] {
  const { provider: leadProvider, foreground, phase, jobs, team, agents, now } = input;
  const crew: CrewMember[] = [
    {
      key: "lead",
      provider: leadProvider,
      role: "lead",
      name: t("Lead"),
      state: foreground ? "working" : "idle",
      status: foreground ? phase.toLowerCase() : t("waiting on the team"),
      title: foreground ? phase : t("The lead waits while its helpers work"),
      kind: "lead",
      model: null,
      task: "",
    },
  ];
  // Each helper by its runs: a run is one worker session (a follow-up job
  // continues in the same one). Every run working now, or finished within
  // the last few minutes, gets a robot of its own, so four artists at once
  // are four robots; a helper with none of those is one robot, by its
  // latest job.
  const rank = (job: JobView) => (job.status === "starting" || job.status === "running" ? 2 : job.status === "waiting" ? 1 : 0);
  const better = (job: JobView, known: JobView | undefined) => !known || rank(job) > rank(known) || (rank(job) === rank(known) && job.startedAtUnixMs >= known.startedAtUnixMs);
  const latest = new Map<string, JobView>();
  const runs = new Map<string, Map<string, JobView>>();
  for (const job of jobs ?? []) {
    if (better(job, latest.get(job.worker))) latest.set(job.worker, job);
    const run = job.workerSession ?? job.id;
    const byRun = runs.get(job.worker) ?? new Map<string, JobView>();
    if (better(job, byRun.get(run))) byRun.set(run, job);
    runs.set(job.worker, byRun);
  }
  const names = [...(team?.workers.map((worker) => worker.name) ?? []), ...[...latest.keys()].filter((name) => !team?.workers.some((worker) => worker.name === name))];
  for (const name of names) {
    const slot = team?.workers.find((worker) => worker.name === name);
    const recent = [...(runs.get(name)?.entries() ?? [])].filter(([, job]) => isRecent(job, now)).sort(([, a], [, b]) => a.startedAtUnixMs - b.startedAtUnixMs);
    const shown: Array<[string | null, JobView | undefined]> = recent.length > 0 ? recent : [[null, latest.get(name)]];
    shown.forEach(([run, job], index) => {
      const provider = job?.provider ?? slot?.provider ?? leadProvider;
      const open = job && (job.status === "starting" || job.status === "running");
      const state: BotState = open ? "working" : job && (job.status === "succeeded" || job.status === "failed" || job.status === "cancelled") ? "done" : "idle";
      const status = !job
        ? t("ready")
        : open
          ? elapsed(now - job.startedAtUnixMs)
          : job.status === "waiting"
            ? t("waiting out a limit")
            : job.status === "succeeded"
              ? t("done")
              : job.status === "failed"
                ? t("failed")
                : t("stopped");
      crew.push({
        // The first run keeps the helper's own key, so its robot stays put as others come and go.
        key: index === 0 ? `job:${name}` : `job:${name}:${run}`,
        provider,
        role: slot ? closestRole(slot) : null,
        name: index === 0 ? name : `${name} ${index + 1}`,
        state,
        status,
        title: job ? job.task.split("\n")[0]?.slice(0, 140) ?? name : t("On the team, with nothing to do yet"),
        kind: "helper",
        model: job?.model ?? slot?.model ?? null,
        task: job?.task ?? "",
      });
    });
  }
  for (const agent of agents) {
    const running = agent.status === undefined || agent.status === "working" || agent.status === "starting";
    const failed = agent.outcome === "failed";
    const state: BotState = running ? "working" : agent.status === "idle" ? "idle" : "done";
    crew.push({
      key: `agent:${agent.id}`,
      provider: leadProvider,
      role: null,
      name: agent.name,
      state,
      status: running
        ? typeof agent.generationStartedAtUnixMs === "number"
          ? elapsed(now - agent.generationStartedAtUnixMs)
          : t("working")
        : agent.status === "idle"
          ? t("ready")
          : failed
            ? t("failed")
            : t("done"),
      title: agent.task,
      kind: "subagent",
      model: agent.model ?? null,
      task: agent.task,
    });
  }
  return crew;
}
