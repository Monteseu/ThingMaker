/**
 * Rename folder: renames a project's folder on disk from inside ThingMaker,
 * so its sessions, transcripts and settings follow it instead of being left
 * behind under the old path. Optionally starts an agent in the renamed
 * project to bring its documentation up to date.
 */
import { useEffect, useRef, useState } from "react";
import type { RenameReport } from "@thingmaker/contracts";
import { startingProvider, useStore } from "../store";
import { t } from "../i18n";

/** Why a folder name cannot be used; the desktop checks again before renaming. */
export function folderNameProblem(name: string, current: string): string | null {
  if (!name.trim()) return t("Type a name.");
  if (name !== name.trim()) return t("The name starts or ends with a space.");
  if (/[\\/:*?"<>|]/.test(name) || [...name].some((c) => c.charCodeAt(0) < 32)) return t('A folder name cannot contain \\ / : * ? " < > |');
  if (name === "." || name === ".." || name.endsWith(".")) return t("A folder name cannot end with a dot.");
  if (name === current) return t("That is already its name.");
  return null;
}

/** What the agent is asked to do once the folder has its new name. */
export function docsBrief(report: RenameReport, oldName: string, newName: string): string {
  return t(
    'This project’s folder was just renamed from "{oldName}" to "{newName}" (old path: {oldRoot}, new path: {newRoot}). ThingMaker already moved its own records and the agents’ session history.\n\nPlease review this project’s documentation and configuration for references to the old folder name or path and update them: README files, CLAUDE.md, AGENTS.md, docs folders, setup and build scripts, and project or package names that are meant to match the folder. Leave alone anything that only happens to share the name, generated files, lockfiles, dependencies and git history. When done, summarize what you changed and anything you deliberately left as it was, and why.',
    { oldName, newName, oldRoot: report.oldRoot, newRoot: report.newRoot },
  );
}

export function RenameFolderDialog({ workspaceId, onClose }: { workspaceId: string; onClose: () => void }) {
  const workspace = useStore((s) => s.workspaces.find((entry) => entry.id === workspaceId));
  const renameWorkspaceFolder = useStore((s) => s.renameWorkspaceFolder);
  const openSession = useStore((s) => s.openSession);
  const providers = useStore((s) => s.providers);
  // Open sessions in the project: their agents are working in the folder.
  const live = useStore((s) => Object.values(s.sessions).filter((session) => session.workspaceId === workspaceId && session.projection.process !== "exited").length);
  const root = workspace?.canonicalRoot ?? "";
  const current = (workspace?.displayPath ?? root).split(/[\\/]/).filter(Boolean).pop() ?? "";
  const [name, setName] = useState(current);
  const [withAgent, setWithAgent] = useState(true);
  const [working, setWorking] = useState(false);
  const [report, setReport] = useState<RenameReport | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const provider = startingProvider(providers);
  const agentReady = !!providers?.find((entry) => entry.provider === provider)?.resolved;

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  const problem = folderNameProblem(name, current);
  const run = async () => {
    if (problem || live > 0) return;
    setWorking(true);
    const done = await renameWorkspaceFolder(workspaceId, name);
    setWorking(false);
    if (!done) return;
    if (withAgent && agentReady && done.problems.length === 0) {
      void openSession(workspaceId, { mode: "new" }, provider, { message: docsBrief(done, current, name) });
      onClose();
      return;
    }
    setReport(done);
  };

  const startAgent = () => {
    if (!report) return;
    void openSession(workspaceId, { mode: "new" }, provider, { message: docsBrief(report, current, name) });
    onClose();
  };

  return (
    <div className="modal-backdrop" onMouseDown={(event) => event.target === event.currentTarget && !working && onClose()} role="presentation">
      <div aria-labelledby="rename-folder-title" aria-modal="true" className="modal wipe-dialog" role="dialog">
        {report ? (
          <>
            <h2 id="rename-folder-title">{t("Folder renamed")}</h2>
            <p className="small">
              {t("{name} now lives in {path}. Its sessions, Claude Code transcripts ({moved} moved) and agent settings ({entries} entries) follow it.", {
                name,
                path: report.newRoot,
                moved: report.claudeTranscriptsMoved,
                entries: report.configEntriesUpdated,
              })}
            </p>
            {report.problems.length > 0 && (
              <div className="small chip-warn wipe-problems">
                <p>{t("Some things could not be updated:")}</p>
                <ul>
                  {report.problems.slice(0, 6).map((line) => (
                    <li className="mono" key={line}>{line}</li>
                  ))}
                </ul>
              </div>
            )}
            <div className="row wrap">
              {agentReady && (
                <button className="button button-primary" onClick={startAgent} type="button">
                  {t("Update the docs with an agent")}
                </button>
              )}
              <button className="button" onClick={onClose} type="button">
                {t("Close")}
              </button>
            </div>
          </>
        ) : (
          <>
            <h2 id="rename-folder-title">{t("Rename the {name} folder", { name: current })}</h2>
            <p className="small">
              {t("Renames the folder on disk and moves everything that points at it: this project's sessions in ThingMaker, Claude Code's saved conversations, the per-project settings of Claude Code and Codex, and git worktree links.")}
            </p>
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void run();
              }}
            >
              <label className="field">
                <span className="small">{t("New folder name")}</span>
                <input aria-invalid={!!problem && name !== current} className="input mono" disabled={working} onChange={(event) => setName(event.target.value)} ref={input} spellCheck={false} value={name} />
              </label>
              {problem && name !== current && <p className="small chip-warn">{problem}</p>}
              <label className="check">
                <input checked={withAgent && agentReady} disabled={!agentReady || working} onChange={(event) => setWithAgent(event.target.checked)} type="checkbox" />
                <span className="small">{t("Then start an agent to find and update the old name in the project's documentation")}</span>
              </label>
              {live > 0 && (
                <p className="small chip-warn">
                  {live === 1 ? t("1 session is still open. Stop it first.") : t("{count} sessions are still open. Stop them first.", { count: live })}
                </p>
              )}
              <p className="small muted">{t("Close any editor, terminal or Explorer window using the folder first, or the system will not let it be renamed.")}</p>
              <div className="row wrap">
                <button className="button button-primary" disabled={working || !!problem || live > 0} type="submit">
                  {working ? t("Renaming…") : t("Rename folder")}
                </button>
                <button className="button" disabled={working} onClick={onClose} type="button">
                  {t("Cancel")}
                </button>
              </div>
            </form>
          </>
        )}
      </div>
    </div>
  );
}
