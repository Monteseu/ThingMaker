/**
 * Ultra-Review in the renderer: the panel a review starts with, what a
 * review costs in turns, and how its debate reads (who stands where, which
 * phase it is in). Pure functions; the screens are in components/UltraReview*.
 */
import { PROVIDER_LABELS, UR_LENSES, type JobView, type Provider, type ProviderModel, type TeamPreset, type UltraReview, type UrConfig, type UrEntryKind, type UrLens, type UrReviewer, type UrSeat, type UrPullRequest, type UrStanding, type WorkerSlot } from "@thingmaker/contracts";
import { t } from "./i18n";
import { newPreset, uniqueName } from "./team";
import { ROLE_NOTES, closestRole, modelName, type Catalogs } from "./teamRoles";

export const DEFAULT_CONFIG: UrConfig = { quick: false, rounds: 2, requireEvidence: true, allowTests: true };
export const MIN_SEATS = 2;
export const MAX_SEATS = 6;

export function lensLabel(lens: UrLens): string {
  switch (lens) {
    case "architecture":
      return t("Architecture");
    case "correctness":
      return t("Correctness");
    case "tests":
      return t("Tests");
    case "security":
      return t("Security");
    case "performance":
      return t("Performance");
    case "readability":
      return t("Readability");
    case "devils_advocate":
      return t("Devil's advocate");
    default:
      return t("General");
  }
}

export function lensBlurb(lens: UrLens): string {
  switch (lens) {
    case "architecture":
      return t("Structure and design: whether the change fits the codebase and will age well.");
    case "correctness":
      return t("Edge cases, state and concurrency. Backs every bug it claims with a failing test.");
    case "tests":
      return t("What the tests really prove, and what they would miss.");
    case "security":
      return t("Injection, secrets, permissions, unsafe input.");
    case "performance":
      return t("Work per call or render, timers, memory, anything that grows with the data.");
    case "readability":
      return t("Naming, clarity, dead code, comments that mislead.");
    case "devils_advocate":
      return t("Argues against the strongest claims, so nothing passes on agreement alone.");
    default:
      return t("Anything that matters to the author.");
  }
}

function find(catalogs: Catalogs, provider: Provider, pattern: RegExp): ProviderModel | undefined {
  return catalogs[provider]?.find((model) => pattern.test(`${model.id} ${model.name}`));
}

function seat(provider: Provider, model: ProviderModel | undefined, lens: UrLens, chair = false): UrSeat {
  const effort = model?.efforts.includes("high") ? "high" : model?.defaultEffort;
  return { provider, ...(model ? { model: model.id } : {}), ...(effort ? { effort } : {}), lens, chair };
}

/**
 * The panel a review starts with, on the providers that can run: a Claude
 * Chair on architecture, Codex on correctness, Gemini on tests, and a second
 * Claude as devil's advocate; fewer providers, the same lenses doubled up.
 */
export function defaultPanel(catalogs: Catalogs, usable: readonly Provider[]): UrSeat[] {
  const has = (provider: Provider) => usable.includes(provider);
  const seats: UrSeat[] = [];
  const first = (...tries: Array<[Provider, RegExp]>) => {
    for (const [provider, pattern] of tries) if (has(provider)) return { provider, model: find(catalogs, provider, pattern) ?? catalogs[provider]?.find((model) => model.isDefault) };
    return null;
  };
  const chair = first(["claude", /opus/i], ["codex", /sol/i], ["gemini", /pro/i]);
  if (!chair) return [];
  seats.push(seat(chair.provider, chair.model, "architecture", true));
  const correctness = first(["codex", /sol/i], ["claude", /sonnet/i], ["gemini", /pro/i]);
  if (correctness) seats.push(seat(correctness.provider, correctness.model, "correctness"));
  const tests = first(["gemini", /pro/i], ["codex", /sol/i], ["claude", /sonnet/i]);
  if (tests && (tests.provider !== correctness?.provider || usable.length === 1)) seats.push(seat(tests.provider, tests.model, "tests"));
  const devil = first(["claude", /sonnet/i], ["codex", /./], ["gemini", /./]);
  if (devil) seats.push(seat(devil.provider, devil.model, "devils_advocate"));
  return seats.slice(0, MAX_SEATS);
}

/** A helper's name says its lens when the team was saved from a panel
 *  (`correctness`, `devils-advocate-2`); otherwise none. */
function lensOfName(name: string): UrLens | null {
  const id = name.trim().toLowerCase().replace(/-\d+$/, "").replace(/[-\s]+/g, "_");
  return UR_LENSES.find((lens) => lens === id) ?? null;
}

/** A saved team as a panel: its lead chairs, its helpers review by the lens
 *  their name gives (a team saved from a panel) or else by their role. */
export function panelFromPreset(preset: TeamPreset): UrSeat[] {
  const lead = preset.orchestrator;
  const seats: UrSeat[] = [{ provider: lead.provider, ...(lead.model ? { model: lead.model } : {}), ...(lead.effort ? { effort: lead.effort } : {}), lens: "architecture", chair: true }];
  const roleNotes = Object.values(ROLE_NOTES);
  for (const worker of preset.combo.workers) {
    const role = closestRole(worker);
    const lens: UrLens = lensOfName(worker.name) ?? (role === "reviewer" ? "correctness" : role === "coder" ? "tests" : role === "researcher" ? "security" : "general");
    const focus = worker.prompt?.trim() || (worker.note && !roleNotes.includes(worker.note) ? worker.note : "");
    seats.push({ provider: worker.provider, ...(worker.model ? { model: worker.model } : {}), ...(worker.effort ? { effort: worker.effort } : {}), lens, chair: false, ...(focus ? { focus } : {}) });
  }
  return seats.slice(0, MAX_SEATS);
}

/** A panel as a saved team: the Chair leads, the others become reviewer
 *  helpers named by their lens, with their focus as standing instructions. */
export function presetFromPanel(seats: UrSeat[], name: string, takenNames: string[]): TeamPreset {
  const chair = seats.find((entry) => entry.chair) ?? seats[0];
  const preset = newPreset(name, takenNames);
  if (!chair) return preset;
  const names: string[] = [];
  const workers: WorkerSlot[] = seats
    .filter((entry) => entry !== chair)
    .map((entry) => {
      const workerName = uniqueName(entry.lens.replace(/_/g, "-"), names);
      names.push(workerName);
      return {
        name: workerName,
        provider: entry.provider,
        ...(entry.model ? { model: entry.model } : {}),
        ...(entry.effort ? { effort: entry.effort } : {}),
        capabilities: ["review"],
        note: ROLE_NOTES.reviewer,
        ...(entry.focus?.trim() ? { prompt: entry.focus.trim() } : {}),
      };
    });
  return { ...preset, orchestrator: { provider: chair.provider, ...(chair.model ? { model: chair.model } : {}), ...(chair.effort ? { effort: chair.effort } : {}) }, combo: { workers, nativeSubagents: false } };
}

/** Whether a search names one pull request: `#12`, `12`, or a link to it. */
export function isPrLookup(query: string): boolean {
  return /^#?\d+$/.test(query.trim()) || /\/pull\/\d+/.test(query);
}

/** Whether a listed pull request matches what is typed: its number, or words
 *  in its title, branches or author. */
export function matchesPr(pr: UrPullRequest, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (/^#?\d+$/.test(text)) return String(pr.number).startsWith(text.replace(/^#/, ""));
  const haystack = `${pr.title} ${pr.headRefName} ${pr.baseRefName} ${pr.author?.login ?? ""}`.toLowerCase();
  return text.split(/\s+/).every((word) => haystack.includes(word));
}

/** Makes `index` the Chair, and nobody else. */
export function withChair(seats: UrSeat[], index: number): UrSeat[] {
  return seats.map((entry, at) => ({ ...entry, chair: at === index }));
}

/** About how many agent turns a review takes: blind reviews, each cycle's
 *  cross-examination and (roughly half the panel's) rebuttals, the verdict. */
export function estimateTurns(seats: number, config: UrConfig): number {
  if (seats === 0) return 0;
  const cycles = config.quick ? 1 : Math.min(3, Math.max(1, config.rounds));
  const rebuttals = config.quick ? 0 : Math.ceil(seats / 2);
  return seats + cycles * (seats + rebuttals) + 1;
}

/** Turns by provider, for the usage line. */
export function turnsByProvider(seats: UrSeat[], config: UrConfig): Partial<Record<Provider, number>> {
  const total = estimateTurns(seats.length, config);
  const share: Partial<Record<Provider, number>> = {};
  for (const entry of seats) share[entry.provider] = (share[entry.provider] ?? 0) + 1;
  const result: Partial<Record<Provider, number>> = {};
  for (const [provider, count] of Object.entries(share) as Array<[Provider, number]>) {
    const chairBonus = seats.some((entry) => entry.chair && entry.provider === provider) ? 1 : 0;
    result[provider] = Math.round(((total - 1) * count) / seats.length) + chairBonus;
  }
  return result;
}

export function seatName(catalogs: Catalogs, entry: Pick<UrSeat, "provider" | "model">): string {
  return modelName(catalogs, entry.provider, entry.model) ?? PROVIDER_LABELS[entry.provider];
}

/** How a reviewer is named on screen: by letter while the debate runs, by
 *  model once it is over (identities stay hidden from the reviewers, and
 *  from the screen while it matters). */
export function reviewerName(review: UltraReview, reviewer: UrReviewer, catalogs: Catalogs): string {
  if (review.status === "running" || review.status === "interrupted") return t("Reviewer {id}", { id: reviewer.id });
  return seatName(catalogs, reviewer);
}

export type Stance = "author" | "confirm" | "refine" | "challenge" | "none";

/** Each reviewer's latest word on each finding. */
export function stanceMatrix(review: UltraReview): Array<{ finding: string; title: string; stances: Record<string, Stance> }> {
  return review.findings.map((finding) => {
    const stances: Record<string, Stance> = {};
    for (const reviewer of review.reviewers) stances[reviewer.id] = reviewer.id === finding.author ? "author" : "none";
    for (const entry of finding.thread) {
      if (entry.by === finding.author || !(entry.kind === "confirm" || entry.kind === "refine" || entry.kind === "challenge")) continue;
      stances[entry.by] = entry.kind;
    }
    return { finding: finding.id, title: finding.title, stances };
  });
}

export function standingLabel(standing: UrStanding, settled: boolean): string {
  switch (standing) {
    case "confirmed":
      return settled ? t("Confirmed") : t("Leaning confirmed");
    case "contested":
      return t("Contested");
    case "challenged":
      return settled ? t("Refuted") : t("Challenged");
    case "withdrawn":
      return t("Withdrawn");
    default:
      return t("Open");
  }
}

export function entryLabel(kind: UrEntryKind): string {
  switch (kind) {
    case "claim":
      return t("claim");
    case "confirm":
      return t("confirms");
    case "challenge":
      return t("challenges");
    case "refine":
      return t("refines");
    case "defend":
      return t("defends");
    case "accept":
      return t("accepts");
    case "withdraw":
      return t("withdraws");
    default:
      return t("note");
  }
}

/** Where the review is, in a few words, for the tab and the rounds strip. */
export function phaseLabel(review: UltraReview): string {
  if (review.status === "interrupted") return t("interrupted");
  if (review.status === "cancelled") return t("stopped");
  if (review.status === "failed") return t("failed");
  switch (review.phase) {
    case "blind":
      return t("blind review");
    case "cross":
      return t("cross-examination · round {round}", { round: review.round });
    case "rebuttal":
      return t("rebuttal · round {round}", { round: review.round });
    case "verdict":
      return t("writing the verdict");
    default:
      return t("verdict");
  }
}

export type PhaseStep = { id: "blind" | "cross" | "rebuttal" | "verdict"; label: string; state: "done" | "now" | "next" | "skipped" };

/** The strip of phases across the debate. */
export function phaseSteps(review: UltraReview): PhaseStep[] {
  const order = ["blind", "cross", "rebuttal", "verdict"] as const;
  const at = review.phase === "done" ? order.length : order.indexOf(review.phase);
  const labels: Record<(typeof order)[number], string> = {
    blind: t("Blind review"),
    cross: t("Cross-examination"),
    rebuttal: t("Rebuttal"),
    verdict: t("Verdict"),
  };
  return order.map((id, index) => ({
    id,
    label: labels[id],
    state: id === "rebuttal" && review.config.quick ? "skipped" : index < at ? "done" : index === at && review.status === "running" ? "now" : "next",
  }));
}

/** A reviewer's newest job, for what it is doing right now. */
export function reviewerJob(jobs: readonly JobView[] | undefined, reviewer: UrReviewer): JobView | undefined {
  const name = `Reviewer ${reviewer.id}`;
  return [...(jobs ?? [])].filter((job) => job.worker === name).sort((a, b) => b.startedAtUnixMs - a.startedAtUnixMs)[0];
}

export type VerdictCounts = { confirmed: number; split: number; refuted: number; withdrawn: number };

export function verdictCounts(review: UltraReview): VerdictCounts {
  const counts: VerdictCounts = { confirmed: 0, split: 0, refuted: 0, withdrawn: 0 };
  for (const item of review.verdict?.items ?? []) counts[item.outcome] += 1;
  return counts;
}

/** The confirmed findings (and the splits the user chose to fix), as a
 *  message the session's own agent can act on. */
export function fixRequest(review: UltraReview): string | null {
  const items = (review.verdict?.items ?? []).filter((item) => item.outcome === "confirmed" || (item.outcome === "split" && item.decision === "fix"));
  if (items.length === 0) return null;
  const lines = items.map((item) => {
    const finding = review.findings.find((entry) => entry.id === item.finding);
    const where = finding?.file ? ` (${finding.file}${finding.line ? `:${finding.line}` : ""})` : "";
    const fix = item.fix ? `\n  Suggested fix: ${item.fix}` : "";
    return `- [${item.severity ?? finding?.severity ?? "medium"}] ${finding?.title ?? item.finding}${where}: ${item.summary}${fix}`;
  });
  const target = review.target.kind === "pr" ? `pull request #${review.target.number}` : `the ${review.target.headRef} branch`;
  return `An Ultra-Review of ${target} confirmed these findings. Fix them, then run the tests:\n\n${lines.join("\n")}`;
}
