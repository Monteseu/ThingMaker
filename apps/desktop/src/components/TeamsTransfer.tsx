/**
 * The Teams view's Import and Export: teams and saved helpers as JSON files
 * (see teamFile). Export picks saved teams and helper presets and saves them
 * to a file; Import reads a file and adds what it holds beside what is here.
 */
import { useEffect, useRef, useState, type ChangeEvent } from "react";
import { api } from "../ipc";
import { useStore } from "../store";
import { exportFileName, exportTeams, importedHelpers, importedPresets, readTeamsFile } from "../teamFile";
import { t } from "../i18n";

/** "2 teams and 1 helper", for what was exported or imported. */
function amount(teams: number, helpers: number): string {
  const teamText = teams === 1 ? t("1 team") : t("{count} teams", { count: teams });
  const helperText = helpers === 1 ? t("1 helper") : t("{count} helpers", { count: helpers });
  if (helpers === 0) return teamText;
  if (teams === 0) return helperText;
  return t("{teams} and {helpers}", { teams: teamText, helpers: helperText });
}

export function TeamsTransfer({ onImported }: { onImported: (firstId: string) => void }) {
  const presets = useStore((s) => s.teamPresets);
  const helpers = useStore((s) => s.helperPresets);
  const addTeamPresets = useStore((s) => s.addTeamPresets);
  const addHelperPresets = useStore((s) => s.addHelperPresets);
  const loadHelperPresets = useStore((s) => s.loadHelperPresets);
  const [open, setOpen] = useState(false);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [note, setNote] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const file = useRef<HTMLInputElement>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  useEffect(() => {
    void loadHelperPresets();
  }, [loadHelperPresets]);

  // The note says what happened, then goes.
  useEffect(() => {
    if (!note) return;
    const timer = setTimeout(() => setNote(null), note.kind === "ok" ? 4000 : 9000);
    return () => clearTimeout(timer);
  }, [note]);

  const toggle = (id: string) => {
    const next = new Set(chosen);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setChosen(next);
  };
  const all = presets.length > 0 && presets.every((preset) => chosen.has(preset.id));
  const allHelpers = helpers.length > 0 && helpers.every((preset) => chosen.has(preset.id));
  const picked = presets.filter((preset) => chosen.has(preset.id));
  const pickedHelpers = helpers.filter((preset) => chosen.has(preset.id));
  const pickAll = (ids: string[], on: boolean) => {
    const next = new Set(chosen);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    setChosen(next);
  };

  const save = async () => {
    try {
      const path = await api.saveTextFile(exportFileName(picked, pickedHelpers), exportTeams(picked, pickedHelpers));
      if (path === null) return;
      setOpen(false);
      setNote({ kind: "ok", text: picked.length === 1 && pickedHelpers.length === 0 ? t("Exported “{name}”.", { name: picked[0]?.name ?? "" }) : t("Exported {what}.", { what: amount(picked.length, pickedHelpers.length) }) });
    } catch (error) {
      setNote({ kind: "error", text: error instanceof Error ? error.message : String(error) });
    }
  };

  const load = async (event: ChangeEvent<HTMLInputElement>) => {
    const picked = event.target.files?.[0];
    event.target.value = "";
    if (!picked) return;
    try {
      const { teams: entries, helpers: helperEntries } = readTeamsFile(await picked.text());
      const added = importedPresets(entries, useStore.getState().teamPresets);
      const addedHelpers = importedHelpers(helperEntries, useStore.getState().helperPresets);
      if (added.length > 0 && !(await addTeamPresets(added))) return;
      if (addedHelpers.length > 0 && !(await addHelperPresets(addedHelpers))) return;
      const renamed = added.filter((preset, index) => preset.name !== entries[index]?.name).length + addedHelpers.filter((preset, index) => preset.name !== helperEntries[index]?.name).length;
      setNote({
        kind: "ok",
        text:
          (added.length === 1 && addedHelpers.length === 0 ? t("Imported “{name}”.", { name: added[0]?.name ?? "" }) : t("Imported {what}.", { what: amount(added.length, addedHelpers.length) })) +
          (renamed > 0 ? ` ${t("Names already in use got a number.")}` : ""),
      });
      if (added[0]) onImported(added[0].id);
    } catch (error) {
      setNote({ kind: "error", text: error instanceof Error ? error.message : String(error) });
    }
  };

  return (
    <div className="teams-transfer" ref={root}>
      {note && (
        <span className={`small teams-transfer-note ${note.kind === "error" ? "teams-transfer-error" : ""}`} role={note.kind === "error" ? "alert" : "status"}>
          {note.text}
        </span>
      )}
      <button className="button" onClick={() => file.current?.click()} title={t("Add teams from a JSON file")} type="button">
        {t("Import…")}
      </button>
      <input accept=".json,application/json" aria-label={t("Team file to import")} className="teams-transfer-file" onChange={(event) => void load(event)} ref={file} type="file" />
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        className="button"
        disabled={presets.length === 0 && helpers.length === 0}
        onClick={() => {
          if (!open) setChosen(new Set([...presets, ...helpers].map((preset) => preset.id)));
          setOpen(!open);
        }}
        title={presets.length === 0 && helpers.length === 0 ? t("No teams or helpers to export yet.") : t("Save teams and helpers to a JSON file")}
        type="button"
      >
        {t("Export…")}
      </button>
      {open && (
        <div aria-label={t("Export teams")} className="teams-transfer-pop" role="dialog">
          {presets.length > 0 && (
            <>
              <span className="model-menu-caption">{t("Teams to export")}</span>
              <label className="teams-transfer-row teams-transfer-all">
                <input checked={all} onChange={() => pickAll(presets.map((preset) => preset.id), !all)} type="checkbox" />
                {t("All teams")}
              </label>
              <div className="teams-transfer-list">
                {presets.map((preset) => (
                  <label className="teams-transfer-row" key={preset.id}>
                    <input checked={chosen.has(preset.id)} onChange={() => toggle(preset.id)} type="checkbox" />
                    <span className="teams-transfer-name">{preset.name}</span>
                  </label>
                ))}
              </div>
            </>
          )}
          {helpers.length > 0 && (
            <>
              <span className="model-menu-caption">{t("Helper presets to export")}</span>
              <label className="teams-transfer-row teams-transfer-all">
                <input checked={allHelpers} onChange={() => pickAll(helpers.map((preset) => preset.id), !allHelpers)} type="checkbox" />
                {t("All helper presets")}
              </label>
              <div className="teams-transfer-list">
                {helpers.map((preset) => (
                  <label className="teams-transfer-row" key={preset.id}>
                    <input checked={chosen.has(preset.id)} onChange={() => toggle(preset.id)} type="checkbox" />
                    <span className="teams-transfer-name">{preset.name}</span>
                  </label>
                ))}
              </div>
            </>
          )}
          <button className="button button-primary" disabled={picked.length === 0 && pickedHelpers.length === 0} onClick={() => void save()} type="button">
            {picked.length === 0 && pickedHelpers.length === 0 ? t("Export") : t("Export {what}", { what: amount(picked.length, pickedHelpers.length) })}
          </button>
        </div>
      )}
    </div>
  );
}
