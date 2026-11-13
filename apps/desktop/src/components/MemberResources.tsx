import { useEffect, useState } from "react";
import type { MemberResourceCatalog, MemberResourceSelection, WorkerSlot } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { t } from "../i18n";
import { SkillConnections } from "./SkillConnections";

/** The workspace is a preview: saved references resolve against the actual
 * job workspace at launch. Selecting a group is a snapshot, never a wildcard. */
export function MemberResources({ worker, onChange }: { worker: WorkerSlot; onChange: (worker: WorkerSlot) => void }) {
  const workspaces = useStore((s) => s.workspaces);
  const account = useStore((s) => `${s.accounts.claude?.find((entry) => entry.active)?.id ?? ""}:${s.accounts.codex?.find((entry) => entry.active)?.id ?? ""}`);
  const [workspaceId, setWorkspaceId] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [catalog, setCatalog] = useState<MemberResourceCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState<"skills" | "mcps">("skills");
  const supported = worker.provider === "claude" || worker.provider === "codex";
  const custom = worker.resources !== undefined;

  useEffect(() => {
    if (!supported || !custom) return;
    let cancelled = false;
    setCatalog(null);
    setError(null);
    void api.memberResources(workspaceId || null, worker.provider).then(
      (value) => { if (!cancelled) setCatalog(value); },
      (failure: unknown) => { if (!cancelled) setError(typeof failure === "object" && failure !== null && "message" in failure ? String(failure.message) : String(failure)); },
    );
    return () => { cancelled = true; };
  }, [workspaceId, refresh, supported, custom, account, worker.provider]);

  if (!supported && !custom) return null;
  const inherit = () => {
    const { resources: _resources, ...rest } = worker;
    onChange(rest);
  };
  const selection = worker.resources ?? { skills: [], mcps: [] };
  const patch = (next: MemberResourceSelection) => onChange({ ...worker, resources: next });
  const kind = tab === "skills" ? "skill" : "mcp";
  const visible = (catalog?.entries ?? []).filter((entry) => entry.kind === kind && `${entry.name} ${entry.description} ${entry.group} ${entry.source ?? ""}`.toLowerCase().includes(search.toLowerCase()));
  const groups = [...new Set(visible.map((entry) => entry.group))].sort();
  const groupLabel = (group: string) => group === "user" ? t("Local · user") : group === "project" ? t("Local · project") : `${t("Plugin")} · ${group.slice(7)}`;
  const known = new Set(catalog?.entries.map((entry) => entry.id));
  const missing = catalog ? [...selection.skills, ...selection.mcps].filter((id) => !known.has(id)) : [];

  return (
    <section className="member-resources" aria-label={t("Skills and connections")}>
      <div className="row wrap">
        <strong>{t("Skills and connections")}</strong>
        <select className="input" aria-label={t("Resource mode")} value={custom ? "custom" : "inherit"} onChange={(event) => event.target.value === "inherit" ? inherit() : patch({ skills: [], mcps: [] })}>
          <option value="inherit">{t("Inherit configuration")}</option>
          <option value="custom" disabled={!supported}>{t("Custom selection")}</option>
        </select>
      </div>
      {!supported && <p role="alert">{t("This selection requires Claude Code or Codex. Restore inherited configuration to use another provider.")}</p>}
      {custom && supported && <>
        <p className="small muted">{t("Only checked resources are available to this member. An empty selection enables none. The internal team connection stays available.")}</p>
        <label className="field">{t("Preview project")}
          <select className="input" value={workspaceId} onChange={(event) => setWorkspaceId(event.target.value)}>
            <option value="">{t("User resources only")}</option>
            {workspaces.map((workspace) => <option key={workspace.id} value={workspace.id}>{workspace.displayPath || workspace.canonicalRoot}</option>)}
          </select>
        </label>
        <div className="row wrap">
          <button className="button button-small" aria-pressed={tab === "skills"} onClick={() => setTab("skills")} type="button">{t("Skills")} · {selection.skills.length}</button>
          <button className="button button-small" aria-pressed={tab === "mcps"} onClick={() => setTab("mcps")} type="button">MCPs · {selection.mcps.length}</button>
          <button className="button button-small" onClick={() => setRefresh((value) => value + 1)} type="button">{t("Refresh")}</button>
          <input className="input" aria-label={t("Search resources")} placeholder={t("Search resources")} value={search} onChange={(event) => setSearch(event.target.value)} />
        </div>
        {error && <p role="alert">{error}</p>}
        {catalog?.problem && <p role="alert">{catalog.problem}</p>}
        {catalog && <SkillConnections catalog={catalog} selection={selection} onChange={patch} />}
        {!catalog && !error && <p className="small muted">{t("Loading resources…")}</p>}
        {missing.length > 0 && <div role="alert"><p>{t("Some selected resources are missing from this preview. They must be available in the task's project before this member can start.")}</p>{missing.map((id) => <label className="member-resource" key={id}><input type="checkbox" checked onChange={() => patch({ skills: selection.skills.filter((entry) => entry !== id), mcps: selection.mcps.filter((entry) => entry !== id) })} />{id}</label>)}</div>}
        <div className="member-resource-list">
          {groups.map((group) => {
            const entries = visible.filter((entry) => entry.group === group);
            const available = entries.filter((entry) => entry.available).map((entry) => entry.id);
            const all = available.length > 0 && available.every((id) => selection[tab].includes(id));
            return <details key={group} open>
              <summary>{groupLabel(group)} · {entries.length}</summary>
              <button className="button button-small" disabled={available.length === 0} type="button" onClick={() => patch({ ...selection, [tab]: all ? selection[tab].filter((id) => !available.includes(id)) : [...new Set([...selection[tab], ...available])] })}>{all ? t("Deselect group") : t("Select group")}</button>
              {entries.map((entry) => <label className={`member-resource ${entry.available ? "" : "muted"}`} key={entry.id}>
                <input type="checkbox" aria-label={entry.name} checked={selection[tab].includes(entry.id)} disabled={!entry.available && !selection[tab].includes(entry.id)} onChange={(event) => patch({ ...selection, [tab]: event.target.checked ? [...selection[tab], entry.id] : selection[tab].filter((id) => id !== entry.id) })} />
                <span><strong>{entry.name}</strong>{entry.source && <span className="small muted">{entry.source}</span>}<span className="small muted">{entry.problem ?? entry.description}</span></span>
              </label>)}
            </details>;
          })}
          {catalog && visible.length === 0 && <p className="small muted">{t("No matching resources.")}</p>}
        </div>
        <p className="small muted">{t("Changes apply to new tasks. Files, terminal access and plugin hooks are controlled separately.")}</p>
        <details><summary className="small muted">{t("Pilot scope")}</summary>{catalog?.notes.map((note) => <p className="small muted" key={note}>{note}</p>)}</details>
      </>}
    </section>
  );
}
