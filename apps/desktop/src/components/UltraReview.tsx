/**
 * The Ultra-Review tab: set up a panel of reviewers from several providers,
 * watch them cross-examine each other's findings, and read the verdict.
 *
 * The review runs in the host (crates/.../ultrareview); this tab shows the
 * session's reviews as the host reports them, and starts, steers and stops
 * them. One review runs at a time per session; earlier ones stay listed.
 */
import { useEffect, useMemo, useState } from "react";
import { PROVIDERS, PROVIDER_LABELS, UR_LENSES, type Provider, type UltraReview, type UrConfig, type UrPullRequest, type UrSeat } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { usableProviders } from "../teamRoles";
import { DEFAULT_CONFIG, MAX_SEATS, MIN_SEATS, defaultPanel, estimateTurns, isPrLookup, lensBlurb, lensLabel, matchesPr, panelFromPreset, phaseLabel, presetFromPanel, seatName, turnsByProvider, withChair } from "../ultraReview";
import { ProviderBadge } from "./ProviderMark";
import { useCatalogs } from "./TeamChart";
import { UltraReviewAgora, UltraReviewVerdict } from "./UltraReviewDebate";
import { IconX } from "./icons";
import { t } from "../i18n";

const SETUP_KEY = "thingmaker.ultraReview.setup";

type Setup = { kind: "pr" | "branch"; seats: UrSeat[]; config: UrConfig };

/** The last setup used on this machine: a convenience, so failures are ignored. */
function rememberedSetup(): Partial<Setup> {
  try {
    const raw = localStorage.getItem(SETUP_KEY);
    return raw ? (JSON.parse(raw) as Partial<Setup>) : {};
  } catch {
    return {};
  }
}

function rememberSetup(setup: Setup) {
  try {
    localStorage.setItem(SETUP_KEY, JSON.stringify(setup));
  } catch {
    // Remembering is a convenience.
  }
}

function dateOf(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** The tab's label suffix: what a review in this session is doing. */
export function useUltraReviewLabel(sessionId: string): string {
  const host = useStore((s) => s.sessions[sessionId]?.snapshot.agentSessionId ?? null);
  const latest = useStore((s) => (host ? s.ultraReviews[host]?.[0] : undefined));
  if (!latest || (latest.status !== "running" && latest.status !== "interrupted")) return t("Ultra-Review");
  return `${t("Ultra-Review")} · ${phaseLabel(latest)}`;
}

export function UltraReviewPane({ sessionId }: { sessionId: string }) {
  const host = useStore((s) => s.sessions[sessionId]?.snapshot.agentSessionId ?? null);
  const workspaceId = useStore((s) => s.sessions[sessionId]?.workspaceId ?? null);
  const reviews = useStore((s) => (host ? s.ultraReviews[host] : undefined));
  const loadUltraReviews = useStore((s) => s.loadUltraReviews);
  const [selected, setSelected] = useState<string | null>(null);
  const [setting, setSetting] = useState(false);

  useEffect(() => {
    if (host) void loadUltraReviews(host);
  }, [host, loadUltraReviews]);

  if (!host || !workspaceId) {
    return (
      <div className="ur-pane">
        <p className="muted ur-empty">{t("This session is still starting. Ultra-Review opens once the agent has named it.")}</p>
      </div>
    );
  }
  const list = reviews ?? [];
  const live = list.find((review) => review.status === "running" || review.status === "interrupted");
  const shown = setting && !live ? null : (list.find((review) => review.id === selected) ?? live ?? list[0] ?? null);

  return (
    <div className="ur-pane">
      {list.length > 0 && (
        <div className="ur-history">
          <label className="ur-history-pick">
            <span className="small muted">{t("Review")}</span>
            <select
              aria-label={t("Pick a review")}
              className="input"
              onChange={(event) => {
                setSetting(false);
                setSelected(event.target.value);
              }}
              value={shown?.id ?? ""}
            >
              {setting && <option value="">{t("New review")}</option>}
              {list.map((review) => (
                <option key={review.id} value={review.id}>
                  {review.target.kind === "pr" ? `#${review.target.number} ` : ""}
                  {review.target.title} · {dateOf(review.createdAt)} · {phaseLabel(review)}
                </option>
              ))}
            </select>
          </label>
          <span className="ur-spacer" />
          {!live && !setting && (
            <button className="button" onClick={() => setSetting(true)} type="button">
              {t("New review")}
            </button>
          )}
        </div>
      )}
      {shown ? (
        shown.status === "running" || shown.status === "interrupted" ? (
          <UltraReviewAgora review={shown} />
        ) : (
          <UltraReviewVerdict review={shown} sessionId={sessionId} onNew={() => setSetting(true)} />
        )
      ) : (
        <UltraReviewSetup
          host={host}
          onStarted={(id) => {
            setSetting(false);
            setSelected(id);
          }}
          workspaceId={workspaceId}
        />
      )}
    </div>
  );
}

type LooseSeat = Pick<UrSeat, "provider" | "lens" | "chair"> & { model?: string | undefined; effort?: string | undefined; focus?: string | undefined };

/** A seat without its empty fields. */
function tidy(seat: LooseSeat): UrSeat {
  const { model, effort, focus, ...rest } = seat;
  return { ...rest, ...(model ? { model } : {}), ...(effort ? { effort } : {}), ...(focus ? { focus } : {}) };
}

function SeatCard({ seat, index, count, usable, mixed, onChange, onRemove, onChair }: { seat: UrSeat; index: number; count: number; usable: Provider[]; mixed: boolean; onChange: (seat: UrSeat) => void; onRemove: () => void; onChair: () => void }) {
  const catalogs = useCatalogs();
  // A reviewer reads the code, so a local model sits here as an agent.
  const models = (catalogs[seat.provider] ?? []).filter((entry) => seat.provider !== "local" || entry.id.startsWith("@agent/"));
  const model = models.find((entry) => entry.id === seat.model);
  const efforts = model?.efforts ?? [];
  return (
    <div className={`ur-seat ${seat.chair ? "ur-seat-chair" : ""}`}>
      <div className="ur-seat-head">
        <ProviderBadge provider={seat.provider} />
        <strong className="ur-seat-name">{seatName(catalogs, seat)}</strong>
        <button aria-label={t("Remove this reviewer")} className="icon-button icon-button-xs" disabled={count <= MIN_SEATS} onClick={onRemove} title={t("Remove this reviewer")} type="button">
          <IconX size={12} />
        </button>
      </div>
      <label className="ur-field">
        <span>{t("Provider")}</span>
        <select
          className="input"
          onChange={(event) => {
            const provider = event.target.value as Provider;
            // Local Models has no account default: the first agent model is picked.
            const first = provider === "local" ? catalogs.local?.find((entry) => entry.id.startsWith("@agent/"))?.id : undefined;
            onChange(tidy({ ...seat, provider, model: first, effort: undefined }));
          }}
          value={seat.provider}
        >
          {PROVIDERS.map((provider) => (
            <option disabled={!usable.includes(provider)} key={provider} value={provider}>
              {provider === "gemini" ? "Gemini (Antigravity)" : PROVIDER_LABELS[provider]}
              {usable.includes(provider) ? "" : ` (${provider === "local" ? t("Ollama is not running") : t("not signed in")})`}
            </option>
          ))}
        </select>
      </label>
      <label className="ur-field">
        <span>{t("Model")}</span>
        <select
          className="input"
          onChange={(event) => {
            const next = models.find((entry) => entry.id === event.target.value);
            const effort = next?.efforts.includes("high") ? "high" : next?.defaultEffort;
            onChange(tidy({ ...seat, model: event.target.value || undefined, effort }));
          }}
          value={seat.model ?? ""}
        >
          {seat.provider !== "local" && <option value="">{t("Account default")}</option>}
          {seat.provider === "local" && models.length === 0 && <option value="">{t("No installed model can use tools")}</option>}
          {models.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </select>
      </label>
      {efforts.length > 0 && (
        <label className="ur-field">
          <span>{t("Effort")}</span>
          <select className="input" onChange={(event) => onChange(tidy({ ...seat, effort: event.target.value || undefined }))} value={seat.effort ?? ""}>
            <option value="">{t("Default")}</option>
            {efforts.map((effort) => (
              <option key={effort} value={effort}>
                {effort}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="ur-field">
        <span>{t("Lens")}</span>
        <select className="input" onChange={(event) => onChange({ ...seat, lens: event.target.value as UrSeat["lens"] })} title={lensBlurb(seat.lens)} value={seat.lens}>
          {UR_LENSES.map((lens) => (
            <option key={lens} value={lens}>
              {lensLabel(lens)}
            </option>
          ))}
        </select>
      </label>
      <span className="small muted ur-seat-blurb">{lensBlurb(seat.lens)}</span>
      {seat.provider === "local" && (
        <span className={`chip small ${mixed ? "chip-free-not-private" : "chip-private"}`} title={mixed ? t("Cloud reviewers on this panel read what it says, so its work leaves this computer") : t("Every reviewer is local, so the review stays on this computer")}>
          {mixed ? t("free, not private") : t("free · private")}
        </span>
      )}
      <input
        aria-label={t("Reviewer {n}'s focus", { n: index + 1 })}
        className="input ur-focus"
        maxLength={500}
        onChange={(event) => onChange(tidy({ ...seat, focus: event.target.value || undefined }))}
        placeholder={t("Anything to look at closely (optional)")}
        value={seat.focus ?? ""}
      />
      <label className="ur-chair">
        <input checked={seat.chair} name="ur-chair" onChange={onChair} type="radio" />
        {t("Chair: writes the verdict")}
      </label>
    </div>
  );
}

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error && "message" in error) return String((error as { message: unknown }).message);
  return String(error);
}

/** Saves the panel to Teams under a name the user types. */
function SaveAsTeam({ seats }: { seats: UrSeat[] }) {
  const presets = useStore((s) => s.teamPresets);
  const saveTeamPreset = useStore((s) => s.saveTeamPreset);
  const [name, setName] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    if (!saved) return;
    const timer = window.setTimeout(() => setSaved(null), 4000);
    return () => window.clearTimeout(timer);
  }, [saved]);

  const save = async () => {
    if (!name?.trim()) return;
    const preset = presetFromPanel(seats, name, presets.map((entry) => entry.name));
    await saveTeamPreset(preset);
    if (useStore.getState().teamPresets.some((entry) => entry.id === preset.id)) setSaved(preset.name);
    setName(null);
  };

  if (name === null)
    return (
      <>
        {saved && <span className="small muted">{t("Saved to Teams as “{name}”.", { name: saved })}</span>}
        <button className="button ur-save-team" disabled={seats.length < MIN_SEATS} onClick={() => setName(t("Review panel"))} type="button">
          {t("Save as team…")}
        </button>
      </>
    );
  return (
    <form
      className="ur-save-form"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <input
        aria-label={t("Team name")}
        autoFocus
        className="input"
        onChange={(event) => setName(event.target.value)}
        onFocus={(event) => event.target.select()}
        onKeyDown={(event) => {
          if (event.key === "Escape") setName(null);
        }}
        placeholder={t("Team name")}
        value={name}
      />
      <button className="button button-primary" disabled={!name.trim()} type="submit">
        {t("Save")}
      </button>
      <button className="button" onClick={() => setName(null)} type="button">
        {t("Cancel")}
      </button>
    </form>
  );
}

function UltraReviewSetup({ workspaceId, host, onStarted }: { workspaceId: string; host: string; onStarted: (id: string) => void }) {
  const catalogs = useCatalogs();
  const providers = useStore((s) => s.providers);
  const loadProviders = useStore((s) => s.loadProviders);
  const presets = useStore((s) => s.teamPresets);
  const loadTeamPresets = useStore((s) => s.loadTeamPresets);
  const startUltraReview = useStore((s) => s.startUltraReview);
  const busy = useStore((s) => s.busy !== null);
  const usable = useMemo(() => usableProviders(providers), [providers]);
  const remembered = useMemo(rememberedSetup, []);
  const [kind, setKind] = useState<"pr" | "branch">(remembered.kind ?? "pr");
  const [seats, setSeats] = useState<UrSeat[]>(remembered.seats?.length ? remembered.seats : []);
  const [config, setConfig] = useState<UrConfig>({ ...DEFAULT_CONFIG, ...remembered.config });
  // The open pull requests, what a search found, and the one picked.
  const [prs, setPrs] = useState<UrPullRequest[] | null>(null);
  const [prError, setPrError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [searched, setSearched] = useState("");
  const [found, setFound] = useState<UrPullRequest[]>([]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const [selected, setSelected] = useState<UrPullRequest | null>(null);
  const number = selected?.number ?? null;

  useEffect(() => {
    void loadTeamPresets();
    if (!providers) void loadProviders();
  }, [loadTeamPresets, loadProviders, providers]);

  // A panel to start from, once the providers and their models are known.
  useEffect(() => {
    if (seats.length === 0 && providers) {
      const panel = defaultPanel(catalogs, usable);
      if (panel.length >= MIN_SEATS) setSeats(panel);
    }
  }, [catalogs, usable, providers, seats.length]);

  useEffect(() => {
    if (kind !== "pr") return;
    let cancelled = false;
    setPrError(null);
    api
      .ultraReviewPullRequests(workspaceId)
      .then((list) => {
        if (cancelled) return;
        setPrs(list);
        setSelected((current) => current ?? list[0] ?? null);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setPrs([]);
        setPrError(errorText(error));
      });
    return () => {
      cancelled = true;
    };
  }, [kind, workspaceId]);

  // Typing filters the list at once; GitHub is asked after a pause, for the
  // pull requests the list does not hold (older ones, or one a link names).
  useEffect(() => {
    const text = query.trim();
    const timer = window.setTimeout(() => setSearched(text), text ? 350 : 0);
    return () => window.clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    setFound([]);
    setSearchError(null);
    if (kind !== "pr" || !searched || prError) return;
    let cancelled = false;
    setSearching(true);
    api
      .ultraReviewPullRequests(workspaceId, searched)
      .then((list) => {
        if (cancelled) return;
        setFound(list);
        if (isPrLookup(searched) && list.length === 1) setSelected(list[0] ?? null);
      })
      .catch((error: unknown) => {
        if (!cancelled) setSearchError(errorText(error));
      })
      .finally(() => {
        if (!cancelled) setSearching(false);
      });
    return () => {
      cancelled = true;
    };
  }, [kind, workspaceId, searched, prError]);

  const matches = useMemo(() => {
    const local = (prs ?? []).filter((pr) => matchesPr(pr, query));
    return [...local, ...found.filter((pr) => !local.some((entry) => entry.number === pr.number))];
  }, [prs, found, query]);
  // The pick stays in view while a search hides it.
  const shown = selected && !matches.some((pr) => pr.number === selected.number) ? [selected, ...matches] : matches;

  const ready = seats.length >= MIN_SEATS && seats.every((entry) => usable.includes(entry.provider)) && (kind === "branch" || number !== null);
  const turns = estimateTurns(seats.length, config);
  const byProvider = turnsByProvider(seats, config);

  const start = async () => {
    rememberSetup({ kind, seats, config });
    const ok = await startUltraReview({ workspaceId, host, kind, ...(kind === "pr" && number !== null ? { number } : {}), seats, config });
    if (ok) {
      const latest = useStore.getState().ultraReviews[host]?.[0];
      if (latest) onStarted(latest.id);
    }
  };

  return (
    <div className="ur-setup">
      <div>
        <h2 className="ur-title">{t("Ultra-Review")}</h2>
        <p className="muted ur-lede">{t("Several models from different providers review the same change on their own, then challenge each other's findings with evidence. Only what holds up reaches the verdict.")}</p>
      </div>

      <section aria-labelledby="ur-target" className="ur-card">
        <h3 className="ur-cap" id="ur-target">
          {t("What to review")}
        </h3>
        <div className="ur-segmented" role="radiogroup" aria-label={t("What to review")}>
          <button aria-checked={kind === "pr"} className={kind === "pr" ? "on" : ""} onClick={() => setKind("pr")} role="radio" type="button">
            {t("A pull request")}
          </button>
          <button aria-checked={kind === "branch"} className={kind === "branch" ? "on" : ""} onClick={() => setKind("branch")} role="radio" type="button">
            {t("This branch's changes")}
          </button>
        </div>
        {kind === "pr" ? (
          prError ? (
            <p className="small ur-warn">{t("Pull requests could not be listed: {error}", { error: prError })}</p>
          ) : prs === null ? (
            <p className="small muted">{t("Asking GitHub for open pull requests…")}</p>
          ) : (
            <>
              <div className="ur-pr-search">
                <input
                  aria-label={t("Search pull requests")}
                  className="input"
                  onChange={(event) => setQuery(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape" && query) {
                      event.stopPropagation();
                      setQuery("");
                    }
                  }}
                  placeholder={t("Search by title, branch, author or number, or paste a pull request link")}
                  spellCheck={false}
                  type="search"
                  value={query}
                />
                {searching && <span className="small muted">{t("Searching GitHub…")}</span>}
              </div>
              {searchError && <p className="small ur-warn">{searchError}</p>}
              {prs.length === 0 && !query && <p className="small muted">{t("This repository has no open pull requests. Paste a link or a number to review another one.")}</p>}
              {query && !searching && matches.length === 0 && !searchError && <p className="small muted">{t("No pull request matches “{query}”.", { query: query.trim() })}</p>}
              {shown.length > 0 && (
                <div className="ur-prs" role="radiogroup" aria-label={t("Pull requests")}>
                  {shown.map((pr) => (
                    <label className={`ur-pr ${number === pr.number ? "on" : ""}`} key={pr.number}>
                      <input checked={number === pr.number} name="ur-pr" onChange={() => setSelected(pr)} type="radio" />
                      <span className="ur-pr-text">
                        <span className="ur-pr-title">
                          #{pr.number} {pr.title}
                          {pr.isDraft && <span className="chip small">{t("draft")}</span>}
                          {pr.state === "MERGED" && <span className="chip small">{t("merged")}</span>}
                          {pr.state === "CLOSED" && <span className="chip small">{t("closed")}</span>}
                        </span>
                        <span className="small muted mono">
                          {pr.headRefName} → {pr.baseRefName} · {t("{n} files", { n: pr.changedFiles })} · <span className="ur-add">+{pr.additions}</span> <span className="ur-del">−{pr.deletions}</span>
                          {pr.author?.login ? ` · ${pr.author.login}` : ""}
                        </span>
                      </span>
                    </label>
                  ))}
                </div>
              )}
            </>
          )
        ) : (
          <p className="small muted">{t("The current branch against the default branch, with your uncommitted changes. Reviewers work in a copy, so your files are never touched.")}</p>
        )}
      </section>

      <section aria-labelledby="ur-panel" className="ur-card">
        <div className="ur-card-head">
          <h3 className="ur-cap" id="ur-panel">
            {t("The panel")}
          </h3>
          <span className="small muted">{t("{n} reviewers · each brings a lens · they only know each other by letter", { n: seats.length })}</span>
          <span className="ur-spacer" />
          {presets.length > 0 && (
            <select
              aria-label={t("Load the panel from a saved team")}
              className="input ur-load"
              onChange={(event) => {
                const preset = presets.find((entry) => entry.id === event.target.value);
                if (preset) setSeats(panelFromPreset(preset));
                event.target.value = "";
              }}
              value=""
            >
              <option value="">{t("Load from Teams…")}</option>
              {presets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.name}
                </option>
              ))}
            </select>
          )}
          <SaveAsTeam seats={seats} />
        </div>
        <div className="ur-seats">
          {seats.map((entry, index) => (
            <SeatCard
              count={seats.length}
              mixed={seats.some((other) => other.provider !== "local")}
              index={index}
              key={index}
              onChair={() => setSeats(withChair(seats, index))}
              onChange={(next) => setSeats(seats.map((existing, at) => (at === index ? next : existing)))}
              onRemove={() => {
                const rest = seats.filter((_, at) => at !== index);
                setSeats(entry.chair ? withChair(rest, 0) : rest);
              }}
              seat={entry}
              usable={usable}
            />
          ))}
        </div>
        {seats.length < MAX_SEATS && (
          <button
            className="button ur-add-seat"
            disabled={usable.length === 0}
            onClick={() => setSeats([...seats, { provider: usable[seats.length % Math.max(1, usable.length)] ?? "claude", lens: "general", chair: seats.length === 0 }])}
            type="button"
          >
            {t("+ Add reviewer")}
          </button>
        )}
        {usable.length === 0 && providers && <p className="small ur-warn">{t("No provider is signed in. Sign in to one in Providers first.")}</p>}
      </section>

      <section aria-labelledby="ur-format" className="ur-card">
        <h3 className="ur-cap" id="ur-format">
          {t("How the debate runs")}
        </h3>
        <ol className="ur-steps">
          <li>
            <strong>{t("Blind review")}</strong>
            <span className="small muted">{t("Each reviewer reads the change alone, in its own copy. No one sees another's notes.")}</span>
          </li>
          <li>
            <strong>{t("Cross-examination")}</strong>
            <span className="small muted">{t("Every finding goes to the others. Each must confirm, refine or challenge it, with evidence.")}</span>
          </li>
          <li className={config.quick ? "ur-step-off" : ""}>
            <strong>{t("Rebuttal")}</strong>
            <span className="small muted">{config.quick ? t("Skipped in quick mode.") : t("Authors answer the challenges: defend with proof, refine, or withdraw.")}</span>
          </li>
          <li>
            <strong>{t("Verdict")}</strong>
            <span className="small muted">{t("The Chair weighs the evidence, not the votes: confirmed, refuted, or a real disagreement for you to settle.")}</span>
          </li>
        </ol>
        <label className="ur-quick">
          <input checked={config.quick} onChange={(event) => setConfig({ ...config, quick: event.target.checked })} type="checkbox" />
          <span>
            <strong>{t("Quick review")}</strong>
            <span className="small muted"> · {t("one round of cross-examination, no rebuttal, and reviewers read the code without running tests")}</span>
          </span>
        </label>
        <div className="ur-options">
          <label className="check">
            <input checked={config.requireEvidence} onChange={(event) => setConfig({ ...config, requireEvidence: event.target.checked })} type="checkbox" /> {t("A claim needs evidence: a file and line, a test, or a repro")}
          </label>
          <label className="check">
            <input checked={config.allowTests && !config.quick} disabled={config.quick} onChange={(event) => setConfig({ ...config, allowTests: event.target.checked })} type="checkbox" /> {t("Reviewers may run the tests in their copy")}
          </label>
        </div>
        {!config.quick && (
          <div className="ur-rounds">
            <span>{t("Rounds of cross-examination and rebuttal")}</span>
            <div className="ur-segmented" role="radiogroup" aria-label={t("Rounds of cross-examination and rebuttal")}>
              {[1, 2, 3].map((rounds) => (
                <button aria-checked={config.rounds === rounds} className={config.rounds === rounds ? "on" : ""} key={rounds} onClick={() => setConfig({ ...config, rounds })} role="radio" type="button">
                  {rounds}
                </button>
              ))}
            </div>
            <span className="small muted">{t("Stops early once every finding is settled.")}</span>
          </div>
        )}
      </section>

      <div className="ur-start">
        <span className="small muted">
          {t("About {n} agent turns", { n: turns })}
          {Object.keys(byProvider).length > 0 && ` · ${(Object.entries(byProvider) as Array<[Provider, number]>).map(([provider, count]) => `${PROVIDER_LABELS[provider]} ${count}`).join(" · ")}`}
        </span>
        <span className="ur-spacer" />
        <button className="button button-primary" disabled={!ready || busy} onClick={() => void start()} type="button">
          {t("Start Ultra-Review")}
        </button>
      </div>
    </div>
  );
}

export type { UltraReview };
