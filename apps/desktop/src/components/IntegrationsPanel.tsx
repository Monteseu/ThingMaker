/**
 * Integrations, a tab in Settings (CTX-01, CFG-10): the instruction files,
 * skills and MCP servers every agent reads, and the secrets agents are
 * launched with. Each is a fold with a one-line summary; forms open only when
 * asked for.
 *
 * Everything is read from files with its source shown and saved with hash
 * checks and a backup. A save is labelled "applies to the next session"
 * because agents read these files when a session starts; nothing here talks
 * to an agent or spends tokens.
 */
import { Suspense, lazy, useCallback, useEffect, useState } from "react";
import type { ConfigFile, ConfigTarget, ContextSources, InstructionFileName, SkillEntry, SkillScope } from "@thingmaker/contracts";
import { PROVIDER_LABELS } from "@thingmaker/contracts";
import { api } from "../ipc";
import { basenameOf, useStore } from "../store";
import { t } from "../i18n";

import { SkillImport } from "./SkillImport";
import { RuntimeEnvSection } from "./RuntimeEnvSection";
import { McpSection } from "./McpSection";
import { Fold, FoldItem, Select, SettingGroup, SettingRow } from "./settings/controls";

const CodeEditor = lazy(() => import("./MonacoEditor"));

/** A skill scope as the person reads it. */
const scopeLabel = (scope: SkillScope) => (scope === "project" ? t("project") : t("user"));

/** Which provider reads which instruction file, and why both exist. */
const INSTRUCTION_FILES: { file: InstructionFileName; readBy: string }[] = [
  { file: "AGENTS.md", readBy: PROVIDER_LABELS.codex },
  { file: "CLAUDE.md", readBy: PROVIDER_LABELS.claude },
];

function FileEditor({ target, title, onSaved }: { target: ConfigTarget; title: string; onSaved: () => void }) {
  const setError = useStore((s) => s.setError);
  const [file, setFile] = useState<ConfigFile | null>(null);
  const [draft, setDraft] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [validation, setValidation] = useState<string[] | null>(null);
  const [draftTouched, setDraftTouched] = useState(false);

  const load = useCallback(async () => {
    try {
      const loaded = await api.configReadFile(target);
      setFile(loaded);
      setDraft(loaded.content);
      setDraftTouched(false);
      setStatus(null);
      setValidation(loaded.skill && loaded.skill.problems.length > 0 ? loaded.skill.problems : null);
    } catch (error) {
      setError(error);
    }
  }, [JSON.stringify(target), setError]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    if (!file) return;
    try {
      const outcome = await api.configWriteFile(target, file.contentHash ?? null, draft);
      setStatus(t("Saved to {path}. Agents read it when a session starts, so it applies to the next session, not the running one.", { path: outcome.path }));
      setValidation(null);
      onSaved();
      await load();
    } catch (error) {
      setError(error);
    }
  };

  if (!file) return <p className="muted small">{t("Loading {title}…", { title })}</p>;
  const dirty = draftTouched && draft !== file.content;
  return (
    <div className="config-editor">
      <div className="row wrap">
        <strong>{title}</strong>
        <span className="mono small muted">{file.path}</span>
        {!file.exists && <span className="chip small">{t("does not exist yet")}</span>}
        <button className="button button-small" disabled={!dirty} onClick={() => void save()} type="button">
          {t("Save (hash-checked, backup kept)")}
        </button>
        <button className="button button-small" onClick={() => void load()} type="button">
          {t("Reload")}
        </button>
      </div>
      {validation && validation.length > 0 && (
        <ul className="call-list">
          {validation.map((v) => (
            <li className="small chip-warn" key={v}>
              {v}
            </li>
          ))}
        </ul>
      )}
      {status && <p className="small">{status}</p>}
      <div className="config-editor-body">
        <Suspense fallback={<p className="muted small">{t("Loading editor…")}</p>}>
          <CodeEditor
            onChange={(value) => {
              setDraft(value);
              setDraftTouched(true);
            }}
            path={file.path}
            readOnly={false}
            value={draft}
          />
        </Suspense>
      </div>
    </div>
  );
}

export function IntegrationsSettings() {
  const setError = useStore((s) => s.setError);
  const workspaces = useStore((s) => s.workspaces);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [sources, setSources] = useState<ContextSources | null>(null);
  const [editor, setEditor] = useState<{ target: ConfigTarget; title: string } | null>(null);
  const [newSkill, setNewSkill] = useState<{ scope: SkillScope; name: string } | null>(null);
  const [removing, setRemoving] = useState<SkillEntry | null>(null);
  const [keepBackup, setKeepBackup] = useState(true);
  const [removalNote, setRemovalNote] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setSources(workspaceId ? await api.contextSources(workspaceId) : null);
    } catch (error) {
      setError(error);
    }
  }, [workspaceId, setError]);

  const confirmRemove = async () => {
    if (!removing) return;
    try {
      const removed = await api.skillRemove(removing.scope, removing.scope === "project" ? workspaceId : null, removing.directoryName, keepBackup);
      setRemovalNote(
        removed.backup
          ? t("Removed {name}; a copy was kept at {backup}. Sessions started from now on no longer list it.", { name: removed.directoryName, backup: removed.backup })
          : t("Deleted {name} permanently. Sessions started from now on no longer list it.", { name: removed.directoryName }),
      );
      setRemoving(null);
      await reload();
    } catch (error) {
      setError(error);
    }
  };

  const createSkill = () => {
    if (!newSkill) return;
    const name = newSkill.name.trim();
    const owner = newSkill.scope === "project" ? workspaceId : null;
    api
      .skillCreate(newSkill.scope, owner, name)
      .then(() => {
        setEditor({ target: { kind: "skill", scope: newSkill.scope, workspaceId: owner, directoryName: name }, title: `SKILL.md (${newSkill.scope}/${name})` });
        setNewSkill(null);
        void reload();
      })
      .catch(setError);
  };

  useEffect(() => {
    if (!workspaceId && workspaces[0]) setWorkspaceId(workspaces[0].id);
  }, [workspaces, workspaceId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const present = (file: InstructionFileName) => sources?.instructionFiles.some((entry) => entry.depth === 0 && entry.path.endsWith(`/${file}`)) ?? false;
  const presentCount = INSTRUCTION_FILES.filter(({ file }) => present(file)).length;
  const skills = sources?.skills ?? [];

  return (
    <>
      <SettingGroup>
        <SettingRow hint={t("Instructions and project skills belong to one project")} name={t("Project")}>
          <Select
            choices={workspaces.map((w) => ({ value: w.id, label: basenameOf(w.displayPath) }))}
            label={t("Project")}
            onChange={(id) => {
              setWorkspaceId(id);
              setEditor(null);
            }}
            placeholder={t("No projects yet")}
            value={workspaceId}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup note={t("Changes apply to sessions started afterwards.")}>
        <Fold
          hint={t("What every agent reads when a session starts")}
          name={t("Instructions")}
          summary={t("{n} of {total} files", { n: presentCount, total: INSTRUCTION_FILES.length })}
        >
          {INSTRUCTION_FILES.map(({ file, readBy }) => (
            <FoldItem
              actions={
                workspaceId && (
                  <button className="set-button" onClick={() => setEditor({ target: { kind: "instructions", workspaceId, file }, title: file })} type="button">
                    {present(file) ? t("Edit") : t("Create")}
                  </button>
                )
              }
              key={file}
            >
              <span className="set-item-text mono">{file}</span>
              <span className="set-item-note">{present(file) ? t("Read by {provider}", { provider: readBy }) : t("Not created")}</span>
            </FoldItem>
          ))}
          <p className="set-item-note">{t("Each provider reads its own file. Keep the shared rules in one and point the other at it.")}</p>
        </Fold>

        <Fold defaultOpen hint={t("Abilities agents load when a task needs them")} name={t("Skills")} summary={skills.length === 1 ? t("1 skill") : t("{n} skills", { n: skills.length })}>
          {skills.map((skill) => (
            <FoldItem
              actions={
                <>
                  <button
                    className="set-link"
                    onClick={() =>
                      setEditor({
                        target: { kind: "skill", scope: skill.scope, workspaceId: skill.scope === "project" ? workspaceId : null, directoryName: skill.directoryName },
                        title: `SKILL.md (${skill.scope}/${skill.directoryName})`,
                      })
                    }
                    type="button"
                  >
                    {t("Edit")}
                  </button>
                  <button
                    className="set-link set-link-danger"
                    onClick={() => {
                      setKeepBackup(true);
                      setRemoving(skill);
                    }}
                    type="button"
                  >
                    {t("Remove")}
                  </button>
                </>
              }
              key={skill.path}
            >
              <span className="set-item-text" title={skill.description ?? undefined}>
                {skill.name ?? skill.directoryName}
              </span>
              <span className="set-chip">{skill.scope === "project" ? t("This project") : t("Every project")}</span>
              {skill.shadowed && <span className="set-chip set-chip-warn">{t("hidden by the project's own")}</span>}
              {skill.problems.length > 0 && (
                <span className="set-chip set-chip-warn" title={skill.problems.join("; ")}>
                  {t("needs a fix")}
                </span>
              )}
            </FoldItem>
          ))}
          {removalNote && (
            <p className="set-item-note">
              {removalNote}{" "}
              <button className="set-link" onClick={() => setRemovalNote(null)} type="button">
                {t("dismiss")}
              </button>
            </p>
          )}
          {newSkill ? (
            <form
              className="set-inline-form"
              onSubmit={(event) => {
                event.preventDefault();
                if (newSkill.name.trim()) createSkill();
              }}
            >
              <input aria-label={t("Skill folder name")} autoFocus className="set-input" onChange={(e) => setNewSkill({ ...newSkill, name: e.target.value })} placeholder={t("skill-name")} value={newSkill.name} />
              <Select
                choices={[
                  { value: "project", label: t("This project") },
                  { value: "user", label: t("Every project") },
                ]}
                label={t("Where the skill lives")}
                onChange={(scope) => setNewSkill({ ...newSkill, scope: scope as SkillScope })}
                value={newSkill.scope}
              />
              <button className="set-button" disabled={!newSkill.name.trim() || (newSkill.scope === "project" && !workspaceId)} type="submit">
                {t("Create")}
              </button>
              <button className="set-link" onClick={() => setNewSkill(null)} type="button">
                {t("Cancel")}
              </button>
            </form>
          ) : (
            <div className="set-actions">
              <button className="set-button" onClick={() => setNewSkill({ scope: "project", name: "" })} type="button">
                {t("New skill")}
              </button>
              <SkillImport onDone={() => void reload()} workspaceId={workspaceId} />
            </div>
          )}
        </Fold>

        <McpSection workspaceId={workspaceId} />
        <RuntimeEnvSection />
      </SettingGroup>

      {editor && (
        <section className="set-group">
          <FileEditor onSaved={() => void reload()} target={editor.target} title={editor.title} />
          <button className="set-link" onClick={() => setEditor(null)} type="button">
            {t("Close editor")}
          </button>
        </section>
      )}

      {removing && (
        <div aria-modal="true" className="modal-backdrop" role="dialog">
          <div className="modal">
            <h2>{t("Remove skill {name}?", { name: removing.name ?? removing.directoryName })}</h2>
            <p className="small">
              {t("This removes the {scope} skill directory", { scope: scopeLabel(removing.scope) })} <span className="mono">{removing.path.replace(/\/SKILL\.md$/, "")}</span> (
              {removing.resources.length === 0 ? t("1 file") : t("{count} files", { count: removing.resources.length + 1 })}).{" "}
              {t("Agents stop listing it for sessions started afterwards; running sessions keep the catalog they started with.")}
            </p>
            <label className="check small">
              <input checked={keepBackup} onChange={(e) => setKeepBackup(e.target.checked)} type="checkbox" />{" "}
              {t("Keep a copy in the app data folder (skill-backups), so it can be restored by copying it back")}
            </label>
            {!keepBackup && <p className="small chip-warn">{t("Without a backup the files are deleted permanently.")}</p>}
            <div className="row wrap">
              <button className="button button-warn" onClick={() => void confirmRemove()} type="button">
                {keepBackup ? t("Remove (keep backup)") : t("Delete permanently")}
              </button>
              <button className="button" onClick={() => setRemoving(null)} type="button">
                {t("Cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
