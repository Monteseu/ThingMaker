import type { MemberMcpDependency, MemberResourceCatalog, MemberResourceEntry, MemberResourceSelection } from "@thingmaker/contracts";

export type SkillConnection = MemberMcpDependency & { skills: string[]; candidates: MemberResourceEntry[] };

export function skillConnections(catalog: MemberResourceCatalog, selection: MemberResourceSelection): SkillConnection[] {
  const rows = new Map<string, SkillConnection>();
  for (const skill of catalog.entries.filter((entry) => entry.kind === "skill" && selection.skills.includes(entry.id))) {
    for (const dependency of skill.mcpDependencies ?? []) {
      const key = JSON.stringify([dependency.name.toLowerCase(), [...dependency.mcpIds].sort()]);
      const row = rows.get(key);
      if (row) {
        row.skills = [...new Set([...row.skills, skill.name])];
        row.required ||= dependency.required;
        if (dependency.evidence === "declared") row.evidence = "declared";
      } else rows.set(key, { ...dependency, skills: [skill.name], candidates: catalog.entries.filter((entry) => entry.kind === "mcp" && dependency.mcpIds.includes(entry.id)) });
    }
  }
  return [...rows.values()];
}

/** Different origins may define the same runtime server. Explicit activation
 * replaces that conflicting definition, while preserving unrelated connections. */
export function activateMcp(catalog: MemberResourceCatalog, selection: MemberResourceSelection, id: string): MemberResourceSelection {
  const target = catalog.entries.find((entry) => entry.id === id && entry.kind === "mcp" && entry.available);
  if (!target) return selection;
  const conflicts = new Set(catalog.entries.filter((entry) => entry.kind === "mcp" && entry.name === target.name && entry.id !== target.id).map((entry) => entry.id));
  return { ...selection, mcps: [...new Set([...selection.mcps.filter((id) => !conflicts.has(id)), target.id])] };
}

/** Batch activation never chooses among alternatives or displaces a selected
 * definition. Such conflicts require an explicit choice in the individual row. */
export function unambiguousMcps(rows: SkillConnection[], selection: MemberResourceSelection, catalog: MemberResourceCatalog): string[] {
  const targets = rows.filter((row) => !row.candidates.some((entry) => entry.available && selection.mcps.includes(entry.id)))
    .map((row) => row.candidates.filter((entry) => entry.available)).filter((entries) => entries.length === 1).map((entries) => entries[0]!);
  return [...new Set(targets.filter((target) => !targets.some((other) => other.id !== target.id && other.name === target.name)
    && !catalog.entries.some((entry) => entry.kind === "mcp" && entry.name === target.name && entry.id !== target.id && selection.mcps.includes(entry.id))).map((entry) => entry.id))];
}
