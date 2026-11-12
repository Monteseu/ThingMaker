/**
 * Wipe All: deletes for good, from this computer, every Claude Code and
 * Codex session ThingMaker ran in a project. Says exactly what goes and what
 * does not before it is confirmed, and what it did after.
 */
import { useState } from "react";
import type { WipeReport } from "@thingmaker/contracts";
import { useStore } from "../store";
import { t } from "../i18n";

export function WipeDialog({ workspaceId, name, onClose }: { workspaceId: string; name: string; onClose: () => void }) {
  const records = useStore((s) => s.records[workspaceId]);
  const wipeSessions = useStore((s) => s.wipeSessions);
  // Open sessions in the project: their agents would write the files back.
  const live = useStore((s) => Object.values(s.sessions).filter((session) => session.workspaceId === workspaceId && session.projection.process !== "exited").length);
  const [working, setWorking] = useState(false);
  const [report, setReport] = useState<WipeReport | null>(null);

  // Only what ThingMaker started or opened here, never terminal or other-app sessions.
  const ours = (records ?? []).filter((record) => (record.origin === "desktop" || record.origin === "side" || record.origin === "fork") && (record.provider === "claude" || record.provider === "codex"));
  const claude = ours.filter((record) => record.provider === "claude").length;
  const codex = ours.length - claude;

  const run = async () => {
    setWorking(true);
    const done = await wipeSessions(workspaceId);
    setWorking(false);
    if (done) setReport(done);
  };

  return (
    <div className="modal-backdrop" onMouseDown={(event) => event.target === event.currentTarget && !working && onClose()} role="presentation">
      <div aria-labelledby="wipe-title" aria-modal="true" className="modal wipe-dialog" role="dialog">
        {report ? (
          <>
            <h2 id="wipe-title">{t("Sessions deleted")}</h2>
            <p className="small">
              {t("{claude} Claude Code and {codex} Codex sessions from {name} are gone from this computer ({files} files and folders).", {
                claude: report.claudeSessions,
                codex: report.codexSessions,
                name,
                files: report.filesRemoved,
              })}
            </p>
            {report.problems.length > 0 && (
              <div className="small chip-warn wipe-problems">
                <p>{t("Some things could not be removed:")}</p>
                <ul>
                  {report.problems.slice(0, 6).map((problem) => (
                    <li className="mono" key={problem}>{problem}</li>
                  ))}
                </ul>
              </div>
            )}
            <div className="row wrap">
              <button className="button" onClick={onClose} type="button">
                {t("Close")}
              </button>
            </div>
          </>
        ) : (
          <>
            <h2 id="wipe-title">{t("Wipe all sessions in {name}?", { name })}</h2>
            <p className="small">
              {t(
                "Permanently deletes from this computer the {claude} Claude Code and {codex} Codex sessions ThingMaker ran in this project: their transcripts, subagents, generated images, prompt history and ThingMaker's own records of them. This cannot be undone.",
                { claude, codex },
              )}
            </p>
            <p className="small muted">
              {t(
                "Sessions you ran outside ThingMaker (a terminal, Claude desktop, the Codex app) are not touched. Neither is what Anthropic or OpenAI keep on their side, or what your organization's admins can export.",
              )}
            </p>
            {live > 0 && (
              <p className="small chip-warn">
                {live === 1 ? t("1 session is still open. Stop it first.") : t("{count} sessions are still open. Stop them first.", { count: live })}
              </p>
            )}
            <div className="row wrap">
              <button className="button button-warn" disabled={working || live > 0 || ours.length === 0} onClick={() => void run()} type="button">
                {working
                  ? t("Deleting…")
                  : ours.length === 1
                    ? t("Delete 1 session permanently")
                    : t("Delete {count} sessions permanently", { count: ours.length })}
              </button>
              <button className="button" disabled={working} onClick={onClose} type="button">
                {t("Cancel")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
