/**
 * Ultra-Review's two later screens: the agora, where the panel's findings are
 * argued (reviewers named by letter while it runs), and the verdict, where
 * the Chair's decisions are read, the splits settled by the user, and the
 * result posted, copied or handed to the session to fix.
 */
import { useEffect, useMemo, useState } from "react";
import { ultraReviewKey, type UltraReview, type UrFinding, type UrOutcome, type UrReviewer, type UrSeatRecord } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { entryLabel, fixRequest, lensLabel, phaseLabel, phaseSteps, reviewerJob, reviewerName, seatName, standingLabel, stanceMatrix, verdictCounts, type Stance } from "../ultraReview";
import { ProviderBadge } from "./ProviderMark";
import { useCatalogs } from "./TeamChart";
import { t } from "../i18n";

const EMPTY_JOBS: never[] = [];

function elapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

function Target({ review }: { review: UltraReview }) {
  const target = review.target;
  return (
    <div className="ur-target">
      <span className="ur-target-title">
        {target.kind === "pr" ? `#${target.number} ` : ""}
        {target.title}
      </span>
      <span className="small muted mono">
        {target.headRef} → {target.baseRef} · {t("{n} files", { n: target.files })} · <span className="ur-add">+{target.additions}</span> <span className="ur-del">−{target.deletions}</span>
        {review.config.quick ? ` · ${t("quick")}` : ""}
      </span>
    </div>
  );
}

function standingClass(finding: UrFinding): string {
  switch (finding.standing) {
    case "confirmed":
      return "ur-ok";
    case "challenged":
      return finding.settled ? "ur-bad" : "ur-warn-chip";
    case "contested":
      return "ur-warn-chip";
    case "withdrawn":
      return "ur-muted";
    default:
      return "";
  }
}

const STANCE_MARK: Record<Stance, string> = { author: "", confirm: "✓", refine: "~", challenge: "✗", none: "·" };

function Dots({ review, finding }: { review: UltraReview; finding: UrFinding }) {
  const row = stanceMatrix(review).find((entry) => entry.finding === finding.id);
  const others = review.reviewers.filter((reviewer) => reviewer.id !== finding.author && !reviewer.absent);
  const label = others.map((reviewer) => `${reviewer.id}: ${row?.stances[reviewer.id] ?? "none"}`).join(", ");
  return (
    <span aria-label={label} className="ur-dots" role="img">
      {others.map((reviewer) => (
        <span className={`ur-dot ur-dot-${row?.stances[reviewer.id] ?? "none"}`} key={reviewer.id} />
      ))}
    </span>
  );
}

function who(review: UltraReview, by: string, catalogs: ReturnType<typeof useCatalogs>): string {
  if (by === "user") return t("You");
  const reviewer = review.reviewers.find((entry) => entry.id === by);
  return reviewer ? reviewerName(review, reviewer, catalogs) : by;
}

/** A finding's whole exchange, claim first. */
export function Thread({ review, finding }: { review: UltraReview; finding: UrFinding }) {
  const catalogs = useCatalogs();
  return (
    <div className="ur-thread">
      {finding.thread.map((entry, index) => (
        <article className={`ur-msg ur-msg-${entry.kind}`} key={index}>
          <span aria-hidden="true" className={`ur-avatar ${entry.by === "user" ? "ur-avatar-user" : ""}`}>
            {entry.by === "user" ? "✎" : entry.by}
          </span>
          <div className="ur-bubble">
            <div className="ur-who">
              <strong>{who(review, entry.by, catalogs)}</strong>
              <span className="small muted">
                · {entryLabel(entry.kind)}
                {entry.round > 0 ? ` · ${t("round {n}", { n: entry.round })}` : ""}
              </span>
              {entry.severity && entry.kind !== "claim" && <span className="chip small">{entry.severity}</span>}
            </div>
            <p className="ur-text">{entry.text}</p>
            {entry.evidence && (
              <div className="ur-evidence">
                <span className="small muted">{t("Evidence")}</span>
                <pre>{entry.evidence}</pre>
              </div>
            )}
            {entry.fix && (
              <div className="ur-evidence">
                <span className="small muted">{t("Fix")}</span>
                <pre>{entry.fix}</pre>
              </div>
            )}
          </div>
        </article>
      ))}
    </div>
  );
}

function StepIn({ review, finding }: { review: UltraReview; finding: UrFinding }) {
  const noteOnFinding = useStore((s) => s.noteOnFinding);
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  if (!open) {
    return (
      <div className="ur-stepin">
        <span className="small muted">{t("Reviewers read your note in their next turn, and the Chair weighs it.")}</span>
        <button className="button" onClick={() => setOpen(true)} type="button">
          {t("Step in")}
        </button>
      </div>
    );
  }
  return (
    <form
      className="ur-stepin ur-stepin-open"
      onSubmit={(event) => {
        event.preventDefault();
        void noteOnFinding(review.id, finding.id, text).then((ok) => {
          if (ok) {
            setText("");
            setOpen(false);
          }
        });
      }}
    >
      <textarea aria-label={t("Your note on {id}", { id: finding.id })} autoFocus className="input" maxLength={2000} onChange={(event) => setText(event.target.value)} placeholder={t("What the reviewers should know, e.g. “this path only runs in tests”")} rows={3} value={text} />
      <div className="ur-stepin-actions">
        <button className="button" onClick={() => setOpen(false)} type="button">
          {t("Cancel")}
        </button>
        <button className="button button-primary" disabled={!text.trim()} type="submit">
          {t("Add note")}
        </button>
      </div>
    </form>
  );
}

function Activity({ review, reviewer, now }: { review: UltraReview; reviewer: UrReviewer; now: number }) {
  const catalogs = useCatalogs();
  const jobs = useStore((s) => s.jobs[ultraReviewKey(review.id)] ?? EMPTY_JOBS);
  const openWorker = useStore((s) => s.openWorker);
  const job = reviewerJob(jobs, reviewer);
  const active = job && (job.status === "starting" || job.status === "running");
  const state = reviewer.absent
    ? t("dropped out")
    : job?.status === "waiting"
      ? t("waiting out a limit")
      : active
        ? t("working · {time} · {n} tools", { time: elapsed(now - job.startedAtUnixMs), n: job.toolCalls })
        : t("waiting for the others");
  return (
    <div className={`ur-activity ${active ? "ur-activity-on" : ""}`}>
      <ProviderBadge provider={reviewer.provider} />
      <div className="ur-activity-text">
        <span className="ur-activity-name">
          {t("Reviewer {id}", { id: reviewer.id })}
          {reviewer.chair && <span className="chip small ur-chair-chip">{t("Chair")}</span>}
        </span>
        <span className="small muted">
          {seatName(catalogs, reviewer)} · {lensLabel(reviewer.lens)}
        </span>
        <span className="small">{state}</span>
      </div>
      {job?.workerSession && active && (
        <button className="link small" onClick={() => void openWorker(job)} type="button">
          {t("Watch")}
        </button>
      )}
    </div>
  );
}

export function UltraReviewAgora({ review }: { review: UltraReview }) {
  const cancelUltraReview = useStore((s) => s.cancelUltraReview);
  const resumeUltraReview = useStore((s) => s.resumeUltraReview);
  const [selected, setSelected] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (review.status !== "running") return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [review.status]);
  const finding = review.findings.find((entry) => entry.id === selected) ?? review.findings.find((entry) => !entry.settled) ?? review.findings[0];
  const matrix = useMemo(() => stanceMatrix(review), [review]);

  return (
    <div className="ur-agora">
      <div className="ur-agora-head">
        <Target review={review} />
        <span className="ur-spacer" />
        {review.status === "interrupted" && (
          <button className="button button-primary" onClick={() => void resumeUltraReview(review.id)} type="button">
            {t("Resume")}
          </button>
        )}
        <button className="button" onClick={() => void cancelUltraReview(review.id)} type="button">
          {t("Stop review")}
        </button>
      </div>
      {review.status === "interrupted" && <div className="banner banner-warn">{t("ThingMaker closed while this review ran. Resume picks it up where it stopped; reviewers that had answered are not asked again.")}</div>}
      <ol aria-label={t("Phases")} className="ur-phases">
        {phaseSteps(review).map((step) => (
          <li className={`ur-phase ur-phase-${step.state}`} key={step.id}>
            {step.label}
            {step.state === "now" && step.id !== "blind" && step.id !== "verdict" ? ` · ${t("round {n}", { n: review.round })}` : ""}
            {step.state === "skipped" ? ` · ${t("skipped")}` : ""}
          </li>
        ))}
        <li className="ur-phase-note small muted">{t("Reviewers only know each other by letter")}</li>
      </ol>

      <div className="ur-agora-body">
        <section aria-labelledby="ur-findings" className="ur-findings">
          <div className="ur-col-head">
            <h3 className="ur-cap" id="ur-findings">
              {t("Findings")}
            </h3>
            <span className="small muted">{review.findings.length}</span>
          </div>
          {review.findings.length === 0 && <p className="small muted">{review.phase === "blind" ? t("Reviewers are reading the change on their own. Findings appear here as each one finishes.") : t("No reviewer reported a finding.")}</p>}
          {review.findings.map((entry) => (
            <button aria-current={entry.id === finding?.id} className={`ur-finding ${entry.id === finding?.id ? "on" : ""}`} key={entry.id} onClick={() => setSelected(entry.id)} type="button">
              <span className="ur-finding-title">
                <span className="mono small muted">{entry.id}</span> {entry.title}
              </span>
              <span className="ur-finding-meta">
                <span className={`chip small ${standingClass(entry)}`}>{standingLabel(entry.standing, entry.settled)}</span>
                <span className="chip small">{entry.severity}</span>
                <Dots finding={entry} review={review} />
              </span>
            </button>
          ))}
        </section>

        <section aria-labelledby="ur-exchange" className="ur-exchange">
          {finding ? (
            <>
              <div className="ur-exchange-head">
                <h3 className="ur-cap" id="ur-exchange">
                  {t("The exchange")}
                </h3>
                <span className="ur-exchange-title">{finding.title}</span>
                {finding.file && (
                  <span className="small muted mono">
                    {finding.file}
                    {finding.line ? `:${finding.line}` : ""}
                  </span>
                )}
              </div>
              <Thread finding={finding} review={review} />
              {review.status === "running" && finding.standing !== "withdrawn" && <StepIn finding={finding} review={review} />}
            </>
          ) : (
            <div className="ur-waiting">
              <span className="ur-spinner" aria-hidden="true" />
              <p className="muted">{phaseLabel(review)}</p>
            </div>
          )}
        </section>

        <aside aria-label={t("The panel")} className="ur-rail">
          <div className="ur-rail-card">
            <h3 className="ur-cap">{t("Right now")}</h3>
            {review.reviewers.map((reviewer) => (
              <Activity key={reviewer.id} now={now} review={review} reviewer={reviewer} />
            ))}
          </div>
          {matrix.length > 0 && (
            <div className="ur-rail-card">
              <h3 className="ur-cap">{t("Where they stand")}</h3>
              <table aria-label={t("Each reviewer's stance on each finding")} className="ur-matrix">
                <thead>
                  <tr>
                    <th scope="col">{t("Finding")}</th>
                    {review.reviewers.map((reviewer) => (
                      <th key={reviewer.id} scope="col">
                        {reviewer.id}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {matrix.map((row) => (
                    <tr key={row.finding}>
                      <th scope="row" title={row.title}>
                        {row.finding}
                      </th>
                      {review.reviewers.map((reviewer) => {
                        const stance = row.stances[reviewer.id] ?? "none";
                        return (
                          <td className={`ur-stance-${stance}`} key={reviewer.id} title={stance === "author" ? t("raised it") : stance}>
                            {stance === "author" ? <span className="ur-author">{t("author")}</span> : STANCE_MARK[stance]}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
              <span className="small muted">{t("✓ confirms · ~ refines · ✗ challenges · · not yet")}</span>
            </div>
          )}
          <div className="ur-rail-card">
            <h3 className="ur-cap">{t("House rules")}</h3>
            <span className="small muted">{t("No one judges their own finding. A challenge without evidence doesn't count. The Chair weighs proof, not the number of votes.")}</span>
          </div>
          {review.log.length > 0 && (
            <div className="ur-rail-card">
              <h3 className="ur-cap">{t("Notes")}</h3>
              {review.log.map((line, index) => (
                <span className="small muted" key={index}>
                  {line}
                </span>
              ))}
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}

function recommendationText(review: UltraReview): string {
  switch (review.verdict?.recommendation) {
    case "approve":
      return t("Approve");
    case "request_changes":
      return t("Request changes");
    default:
      return t("Approve after fixes");
  }
}

function VerdictItem({ review, finding, outcome }: { review: UltraReview; finding: UrFinding; outcome: UrOutcome }) {
  const decideFinding = useStore((s) => s.decideFinding);
  const item = review.verdict?.items.find((entry) => entry.finding === finding.id);
  const [open, setOpen] = useState(false);
  if (!item) return null;
  return (
    <div className="ur-item">
      <div className="ur-item-head">
        <span className="ur-item-title">{finding.title}</span>
        {item.severity && <span className={`chip small ur-sev-${item.severity}`}>{item.severity}</span>}
        {finding.file && (
          <span className="small muted mono ur-item-where">
            {finding.file}
            {finding.line ? `:${finding.line}` : ""}
          </span>
        )}
      </div>
      <p className="ur-text">{item.summary}</p>
      {outcome === "confirmed" && item.fix && <pre className="ur-fix">{item.fix}</pre>}
      {outcome === "split" && (
        <>
          <div className="ur-sides">
            <div className="ur-side">
              <span className="ur-side-head ur-ok-text">{t("Worth fixing")}</span>
              <span className="small">{item.for ?? "—"}</span>
            </div>
            <div className="ur-side">
              <span className="ur-side-head ur-bad-text">{t("Leave it")}</span>
              <span className="small">{item.against ?? "—"}</span>
            </div>
          </div>
          <div className="ur-decide" role="group" aria-label={t("Your call on {title}", { title: finding.title })}>
            <button aria-pressed={item.decision === "fix"} className={`button ${item.decision === "fix" ? "button-primary" : ""}`} onClick={() => void decideFinding(review.id, finding.id, item.decision === "fix" ? null : "fix")} type="button">
              {t("Fix it")}
            </button>
            <button aria-pressed={item.decision === "leave"} className={`button ${item.decision === "leave" ? "button-primary" : ""}`} onClick={() => void decideFinding(review.id, finding.id, item.decision === "leave" ? null : "leave")} type="button">
              {t("Leave it")}
            </button>
          </div>
        </>
      )}
      <button className="link small" onClick={() => setOpen(!open)} type="button">
        {open ? t("Hide the exchange") : t("Show the exchange ({n})", { n: finding.thread.length })}
      </button>
      {open && <Thread finding={finding} review={review} />}
    </div>
  );
}

function Section({ review, outcome, title, tone }: { review: UltraReview; outcome: UrOutcome; title: string; tone?: string }) {
  const findings = review.findings.filter((finding) => review.verdict?.items.some((item) => item.finding === finding.id && item.outcome === outcome));
  if (findings.length === 0) return null;
  return (
    <section className={`ur-card ${tone ?? ""}`}>
      <h3 className="ur-cap">{title}</h3>
      {findings.map((finding) => (
        <VerdictItem finding={finding} key={finding.id} outcome={outcome} review={review} />
      ))}
    </section>
  );
}

function HeldUp({ review }: { review: UltraReview }) {
  const catalogs = useCatalogs();
  const [records, setRecords] = useState<UrSeatRecord[] | null>(null);
  useEffect(() => {
    api.ultraReviewTrackRecord().then(setRecords, () => setRecords([]));
  }, [review.id]);
  return (
    <section className="ur-card">
      <h3 className="ur-cap">{t("How each reviewer's findings held up")}</h3>
      <div className="ur-held">
        {review.reviewers.map((reviewer) => {
          const raised = review.findings.filter((finding) => finding.author === reviewer.id);
          const confirmed = raised.filter((finding) => review.verdict?.items.some((item) => item.finding === finding.id && item.outcome === "confirmed")).length;
          const record = records?.find((entry) => entry.provider === reviewer.provider && (entry.model ?? null) === (reviewer.model ?? null) && entry.lens === reviewer.lens);
          const share = raised.length === 0 ? 0 : confirmed / raised.length;
          return (
            <div className="ur-held-row" key={reviewer.id}>
              <ProviderBadge provider={reviewer.provider} />
              <span className="ur-held-name">
                {seatName(catalogs, reviewer)} <span className="small muted">· {lensLabel(reviewer.lens)} · {t("Reviewer {id}", { id: reviewer.id })}</span>
                {reviewer.chair && <span className="chip small ur-chair-chip">{t("Chair")}</span>}
              </span>
              <span aria-hidden="true" className="ur-bar">
                <span style={{ width: `${Math.round(share * 100)}%` }} />
              </span>
              <span className="small ur-held-count">{raised.length === 0 ? t("no findings") : t("{confirmed} of {raised}", { confirmed, raised: raised.length })}</span>
              {record && record.reviews > 1 && <span className="small muted ur-held-all">{t("{confirmed} of {raised} over {n} reviews", { confirmed: record.confirmed, raised: record.raised, n: record.reviews })}</span>}
            </div>
          );
        })}
      </div>
    </section>
  );
}

export function UltraReviewVerdict({ review, sessionId, onNew }: { review: UltraReview; sessionId: string; onNew: () => void }) {
  const setDraft = useStore((s) => s.setDraft);
  const setSessionTab = useStore((s) => s.setSessionTab);
  const setError = useStore((s) => s.setError);
  const deleteUltraReview = useStore((s) => s.deleteUltraReview);
  const [posted, setPosted] = useState<"idle" | "posting" | "posted">("idle");
  const [copied, setCopied] = useState(false);
  const counts = verdictCounts(review);
  const fix = fixRequest(review);
  const verdict = review.verdict;

  const post = async () => {
    const confirmed = await api.confirmDialog({
      title: t("Post the review to pull request #{n}?", { n: review.target.number ?? 0 }),
      message: t("It is posted as a review comment from your GitHub account, with the panel's models named at the end."),
      okLabel: t("Post review"),
      cancelLabel: t("Cancel"),
    });
    if (!confirmed) return;
    setPosted("posting");
    try {
      await api.ultraReviewPost(review.id);
      setPosted("posted");
    } catch (error) {
      setPosted("idle");
      setError(error);
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard?.writeText(await api.ultraReviewMarkdown(review.id));
      setCopied(true);
      setTimeout(() => setCopied(false), 2500);
    } catch (error) {
      setError(error);
    }
  };

  if (!verdict) {
    return (
      <div className="ur-verdict">
        <Target review={review} />
        <div className={`banner ${review.status === "failed" ? "banner-error" : "banner-warn"}`}>
          {review.status === "failed" ? t("The review failed: {error}", { error: review.error ?? t("unknown error") }) : t("The review was stopped before its verdict.")}
        </div>
        {review.findings.length > 0 && (
          <section className="ur-card">
            <h3 className="ur-cap">{t("Findings so far")}</h3>
            {review.findings.map((finding) => (
              <details className="ur-item" key={finding.id}>
                <summary className="ur-item-title">
                  {finding.title} <span className="chip small">{standingLabel(finding.standing, finding.settled)}</span>
                </summary>
                <Thread finding={finding} review={review} />
              </details>
            ))}
          </section>
        )}
        <div className="ur-start">
          <button className="button" onClick={() => void deleteUltraReview(review)} type="button">
            {t("Delete")}
          </button>
          <span className="ur-spacer" />
          <button className="button button-primary" onClick={onNew} type="button">
            {t("New review")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="ur-verdict">
      <div className="ur-verdict-head">
        <div className="ur-verdict-title">
          <Target review={review} />
          <h2>
            <span className={`chip ur-rec ur-rec-${verdict.recommendation}`}>{recommendationText(review)}</span> {verdict.headline}
          </h2>
          {verdict.summary && <p className="muted">{verdict.summary}</p>}
          {!verdict.byChair && <p className="small ur-warn">{t("The Chair could not write this verdict, so it is summed up from where each finding stood when the debate ended.")}</p>}
        </div>
        <div className="ur-verdict-actions">
          <button className="button" onClick={() => void copy()} type="button">
            {copied ? t("Copied") : t("Copy as Markdown")}
          </button>
          {fix && (
            <button
              className="button"
              onClick={() => {
                setDraft(sessionId, fix);
                setSessionTab("transcript");
              }}
              title={t("Puts the confirmed findings in this session's prompt, for its agent to fix")}
              type="button"
            >
              {t("Fix in this session")}
            </button>
          )}
          {review.target.kind === "pr" && (
            <button className="button button-primary" disabled={posted !== "idle"} onClick={() => void post()} type="button">
              {posted === "posted" ? t("Posted to #{n}", { n: review.target.number ?? 0 }) : posted === "posting" ? t("Posting…") : t("Post review to #{n}", { n: review.target.number ?? 0 })}
            </button>
          )}
        </div>
      </div>

      <div className="ur-stats">
        <div className="ur-stat">
          <b className="ur-ok-text">{counts.confirmed}</b>
          <span className="small muted">{t("confirmed")}</span>
        </div>
        <div className="ur-stat">
          <b className="ur-warn-text">{counts.split}</b>
          <span className="small muted">{t("need your call")}</span>
        </div>
        <div className="ur-stat">
          <b className="ur-bad-text">{counts.refuted}</b>
          <span className="small muted">{t("refuted")}</span>
        </div>
        <div className="ur-stat">
          <b>{counts.withdrawn}</b>
          <span className="small muted">{t("withdrawn")}</span>
        </div>
      </div>

      {review.findings.length === 0 && <section className="ur-card">{t("No reviewer reported a finding.")}</section>}
      <Section outcome="split" review={review} title={t("Needs your call")} tone="ur-card-warn" />
      <Section outcome="confirmed" review={review} title={t("Confirmed")} />
      <Section outcome="refuted" review={review} title={t("Refuted")} />
      <Section outcome="withdrawn" review={review} title={t("Withdrawn")} />
      <HeldUp review={review} />
      {review.log.length > 0 && (
        <section className="ur-card">
          <h3 className="ur-cap">{t("Notes")}</h3>
          {review.log.map((line, index) => (
            <p className="small muted" key={index}>
              {line}
            </p>
          ))}
        </section>
      )}
      <div className="ur-start">
        <button className="button" onClick={() => void deleteUltraReview(review)} type="button">
          {t("Delete")}
        </button>
        <span className="ur-spacer" />
        <button className="button button-primary" onClick={onNew} type="button">
          {t("New review")}
        </button>
      </div>
    </div>
  );
}
