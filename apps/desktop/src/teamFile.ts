/**
 * Teams as JSON files: export one or more saved teams, and saved helpers
 * (helper presets), and import them back on this machine or another.
 *
 * A file holds the teams' names, leads and helpers, never ids or anything of
 * the machine's: an imported team is a new team, named apart from the ones
 * already here. Reading is forgiving of what it does not know (a newer
 * file's extra fields are ignored) and strict about what a team needs, so a
 * hand-edited file either imports cleanly or says what is wrong with it.
 */
import { PROVIDERS, type Combo, type OrchestratorChoice, type Provider, type TeamPreset, type WorkerSlot } from "@thingmaker/contracts";
import { t } from "./i18n";
import { newPreset, uniqueName, type HelperPreset } from "./team";

export const TEAMS_FORMAT = "thingmaker.teams";
// Older importers would silently discard a member's resource restrictions.
export const TEAMS_VERSION = 2;

/** One team as a file carries it. */
export type TeamFileEntry = { name: string; orchestrator: OrchestratorChoice; combo: Combo };
/** Saved helpers ride along as `helpers`, each a helper as a team has it.
 *  An older ThingMaker ignores them and still imports the teams. */
export type TeamsFile = { format: typeof TEAMS_FORMAT; version: number; exportedAt: string; teams: TeamFileEntry[]; helpers?: WorkerSlot[] };

/** What a file holds once read. */
export type TeamsFileContent = { teams: TeamFileEntry[]; helpers: WorkerSlot[] };

/** The file for these teams and helpers, pretty-printed so it can be read and edited by hand. */
export function exportTeams(presets: readonly TeamPreset[], helpers: readonly HelperPreset[] = [], now = new Date()): string {
  const file: TeamsFile = {
    format: TEAMS_FORMAT,
    version: presets.some((preset) => preset.combo.workers.some((worker) => worker.resources !== undefined)) || helpers.some((helper) => helper.worker.resources !== undefined) ? TEAMS_VERSION : 1,
    exportedAt: now.toISOString(),
    teams: presets.map((preset) => ({ name: preset.name, orchestrator: { ...preset.orchestrator }, combo: { workers: preset.combo.workers.map((worker) => ({ ...worker })), nativeSubagents: preset.combo.nativeSubagents } })),
    ...(helpers.length > 0 ? { helpers: helpers.map((helper) => ({ ...helper.worker, name: helper.name })) } : {}),
  };
  return `${JSON.stringify(file, null, 2)}\n`;
}

/** A file name for the export: the team's own for one team. */
export function exportFileName(presets: readonly TeamPreset[], helpers: readonly HelperPreset[] = []): string {
  if (presets.length === 0 && helpers.length > 0) return "thingmaker-helpers.json";
  if (presets.length !== 1 || helpers.length > 0) return "thingmaker-teams.json";
  const slug = (presets[0]?.name ?? "team")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return `${slug || "team"}.team.json`;
}

const isProvider = (value: unknown): value is Provider => typeof value === "string" && (PROVIDERS as readonly string[]).includes(value);
const text = (value: unknown): string | undefined => (typeof value === "string" && value.trim() ? value.trim() : undefined);

function readLead(value: unknown, where: string): OrchestratorChoice {
  const lead = value as Partial<OrchestratorChoice> | null;
  if (!lead || typeof lead !== "object" || !isProvider(lead.provider)) throw new Error(t("{where}: its lead needs a provider (claude, codex or gemini).", { where }));
  const model = text(lead.model);
  const effort = text(lead.effort);
  return { provider: lead.provider, ...(model ? { model } : {}), ...(effort ? { effort } : {}) };
}

function readHelper(value: unknown, where: string): WorkerSlot {
  const helper = value as Partial<WorkerSlot> | null;
  const name = text(helper?.name);
  if (!helper || typeof helper !== "object" || !name) throw new Error(t("{where}: every helper needs a name.", { where }));
  if (!isProvider(helper.provider)) throw new Error(t("{where}: the helper “{name}” needs a provider (claude, codex or gemini).", { where, name }));
  const model = text(helper.model);
  const effort = text(helper.effort);
  const note = text(helper.note);
  const prompt = text(helper.prompt);
  const resources = helper.resources;
  if (resources !== undefined && (resources === null || typeof resources !== "object" || !Array.isArray(resources.skills) || !Array.isArray(resources.mcps)
    || Object.keys(resources).some((key) => key !== "skills" && key !== "mcps")
    || [...resources.skills, ...resources.mcps].some((id) => typeof id !== "string" || !id.trim()))) {
    throw new Error(t("Invalid member resource selection."));
  }
  const capabilities = Array.isArray(helper.capabilities) ? helper.capabilities.filter((capability): capability is string => typeof capability === "string" && capability.trim() !== "").map((capability) => capability.trim()) : [];
  return { name, provider: helper.provider, ...(model ? { model } : {}), ...(effort ? { effort } : {}), capabilities, ...(note ? { note } : {}), ...(prompt ? { prompt } : {}), ...(resources ? { resources: { skills: [...new Set(resources.skills)], mcps: [...new Set(resources.mcps)] } } : {}) };
}

/** The teams and helpers in a file's text: a teams file, a list of teams, or one team. Throws, saying what is wrong, when it is none of those. */
export function readTeamsFile(raw: string): TeamsFileContent {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(t("That file is not JSON."));
  }
  const file = parsed as Partial<TeamsFile> | null;
  if (file && typeof file === "object" && "format" in file && file.format !== TEAMS_FORMAT) throw new Error(t("That file is not a ThingMaker teams file."));
  if (file && typeof file === "object" && typeof file.version === "number" && file.version > TEAMS_VERSION) throw new Error(t("That file was made by a newer ThingMaker. Update ThingMaker to import it."));
  const list: unknown[] = Array.isArray(parsed) ? parsed : file && typeof file === "object" && Array.isArray(file.teams) ? file.teams : file && typeof file === "object" && "orchestrator" in file ? [file] : [];
  const helperList: unknown[] = file && typeof file === "object" && !Array.isArray(file) && Array.isArray(file.helpers) ? file.helpers : [];
  if (list.length === 0 && helperList.length === 0) throw new Error(t("That file has no teams or helpers in it."));
  const helpers = helperList.map((value, index) => {
    const name = text((value as Partial<WorkerSlot> | null)?.name);
    return readHelper(value, name ? t("Helper preset “{name}”", { name }) : t("Helper preset {n}", { n: index + 1 }));
  });
  const teams = list.map((value, index) => {
    const entry = value as Partial<TeamFileEntry> | null;
    const name = text(entry?.name);
    const where = name ? t("Team “{name}”", { name }) : t("Team {n}", { n: index + 1 });
    if (!entry || typeof entry !== "object" || !name) throw new Error(t("{where}: it needs a name.", { where }));
    const orchestrator = readLead(entry.orchestrator, where);
    const combo = entry.combo as Partial<Combo> | undefined;
    const workers = (Array.isArray(combo?.workers) ? combo.workers : []).map((worker) => readHelper(worker, where));
    const names = new Set<string>();
    for (const worker of workers) {
      if (names.has(worker.name.toLowerCase())) throw new Error(t("{where}: two helpers are called “{name}”.", { where, name: worker.name }));
      names.add(worker.name.toLowerCase());
    }
    return { name, orchestrator, combo: { workers, nativeSubagents: combo?.nativeSubagents === true } };
  });
  return { teams, helpers };
}

/** Imported helpers as new helper presets: fresh ids, and names apart from the ones already here and from each other. */
export function importedHelpers(helpers: readonly WorkerSlot[], existing: readonly HelperPreset[]): HelperPreset[] {
  const taken = existing.map((preset) => preset.name);
  return helpers.map((worker) => {
    const name = uniqueName(worker.name, taken);
    taken.push(name);
    return { id: `helper-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`, name, worker: { ...worker, name }, updatedAt: Date.now() };
  });
}

/** Imported teams as new saved teams: fresh ids, and names apart from the ones already here and from each other. */
export function importedPresets(entries: readonly TeamFileEntry[], existing: readonly TeamPreset[]): TeamPreset[] {
  const taken = existing.map((preset) => preset.name);
  return entries.map((entry) => {
    const name = uniqueName(entry.name, taken);
    taken.push(name);
    return { ...newPreset(name, []), orchestrator: entry.orchestrator, combo: entry.combo };
  });
}
