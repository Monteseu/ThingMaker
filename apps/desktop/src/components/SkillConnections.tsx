import type { MemberResourceCatalog, MemberResourceSelection } from "@thingmaker/contracts";
import { activateMcp, skillConnections, unambiguousMcps } from "../memberResourceDependencies";
import { t } from "../i18n";
import { useStore } from "../store";

export function SkillConnections({ catalog, selection, onChange }: { catalog: MemberResourceCatalog; selection: MemberResourceSelection; onChange: (selection: MemberResourceSelection) => void }) {
  const setView = useStore((s) => s.setView);
  if (selection.skills.length === 0) return null;
  const rows = skillConnections(catalog, selection);
  const batch = unambiguousMcps(rows, selection, catalog);
  const notes = catalog.entries.filter((entry) => selection.skills.includes(entry.id)).flatMap((entry) => (entry.dependencyNotes ?? []).map((note) => `${entry.name}: ${note}`));
  return <section className="skill-connections" aria-label={t("Connections for selected skills")}>
    <div className="row wrap"><strong>{t("Connections for selected skills")}</strong>
      {batch.length > 0 && <button className="button button-small" type="button" onClick={() => onChange(batch.reduce((next, id) => activateMcp(catalog, next, id), selection))}>{t("Activate unambiguous connections")}</button>}
    </div>
    <p className="small muted">{t("Activating a connection adds it only to this member. Nothing is installed or enabled globally.")}</p>
    {rows.length === 0 && <p className="small muted">{t("No MCP dependencies detected. The skill may still need undeclared connections.")}</p>}
    {rows.map((row) => {
      const active = row.candidates.some((entry) => entry.available && selection.mcps.includes(entry.id));
      const key = JSON.stringify([row.name, row.mcpIds]);
      return <div className="skill-connection" key={key} role="group" aria-label={t("MCP dependency: {name}", { name: row.name })}>
        <div className="row wrap"><strong>{row.name}</strong><span className="small muted">{active ? t("Connection active") : row.required ? t("Required connection not selected") : t("Suggested connection not selected")}</span></div>
        <p className="small muted">{t("Used by: {skills}", { skills: row.skills.join(", ") })} · {row.evidence === "declared" ? t("Declared in skill metadata") : t("Detected in skill instructions")}</p>
        {row.candidates.length === 0 && <p className="small muted">{t("No configured MCP matches. Add it in Settings › Integrations, then refresh this selector.")} <button className="link" onClick={() => setView({ kind: "settings", tab: "integrations" })} type="button">{t("Open integration configuration")}</button></p>}
        {row.candidates.filter((entry) => entry.available).length > 1 && !active && <p className="small muted">{t("Several connections match. Choose the definition this member should use.")}</p>}
        {row.candidates.map((entry) => <label className={`member-resource ${entry.available ? "" : "muted"}`} key={entry.id}>
          <input type="checkbox" aria-label={t("Activate {connection} for {dependency}", { connection: `${entry.name} · ${entry.group} · ${entry.source ?? ""}`, dependency: row.name })}
            checked={selection.mcps.includes(entry.id)} disabled={!entry.available && !selection.mcps.includes(entry.id)}
            onChange={(event) => onChange(event.target.checked ? activateMcp(catalog, selection, entry.id) : { ...selection, mcps: selection.mcps.filter((id) => id !== entry.id) })} />
          <span><strong>{entry.name}</strong><span className="small muted">{entry.group === "user" ? t("Local · user") : entry.group === "project" ? t("Local · project") : `${t("Plugin")} · ${entry.group.slice(7)}`} · {entry.source}{entry.target ? ` · ${entry.target}` : ""}</span>{entry.problem && <span className="small muted">{entry.problem}</span>}</span>
        </label>)}
      </div>;
    })}
    {notes.length > 0 && <details><summary className="small muted">{t("Dependency detection notes")}</summary>{notes.map((note, index) => <p className="small muted" key={index}>{note}</p>)}</details>}
  </section>;
}
