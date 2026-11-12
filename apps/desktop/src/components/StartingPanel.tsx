/**
 * A new session while its agent starts: the chat is there at once, so the
 * person sees something happening and can write. A message sent now waits
 * and goes out as soon as the session is ready; then this view gives way to
 * the session itself.
 */
import { useEffect, useRef, type KeyboardEvent } from "react";
import { PROVIDER_LABELS } from "@thingmaker/contracts";
import { useStore } from "../store";
import { ProviderMark } from "./ProviderMark";
import { t } from "../i18n";

export function StartingPanel() {
  const starting = useStore((s) => s.starting);
  const setStartingDraft = useStore((s) => s.setStartingDraft);
  const workspace = useStore((s) => s.workspaces.find((entry) => entry.id === s.starting?.workspaceId));
  const enterSends = useStore((s) => s.uiPrefs.enterSends);
  const input = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    input.current?.focus();
  }, []);

  if (!starting) return null;
  const provider = PROVIDER_LABELS[starting.provider];
  const queue = () => {
    if (starting.draft.trim()) setStartingDraft(starting.draft, true);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
    const sends = enterSends ? !event.shiftKey : event.shiftKey;
    if (!sends) return;
    event.preventDefault();
    queue();
  };

  return (
    <section aria-label={t("New session")} className="panel session-panel starting-panel">
      <header className="session-header">
        <strong>{t("New session")}</strong>
        {workspace && <span className="muted">{workspace.displayPath.split(/[\\/]/).pop()}</span>}
      </header>
      <div aria-busy={!starting.error} className="transcript">
        <div className="starting-line" role="status">
          <ProviderMark provider={starting.provider} size={15} />
          {starting.error ? <span className="chip-warn-text">{t("{provider} could not start: {error}", { provider, error: starting.error })}</span> : <span className="muted">{t("Starting {provider}…", { provider })}</span>}
        </div>
        {starting.queued && (
          <article className="card card-user">
                        <div className="starting-queued">{starting.draft}</div>
            <span className="small muted">{t("Sends as soon as {provider} is ready", { provider })}</span>
          </article>
        )}
      </div>
      <footer className="composer">
        <div className="composer-wrap">
          <textarea
            aria-label={t("Prompt")}
            className="textarea"
            disabled={starting.queued}
            onChange={(event) => setStartingDraft(event.target.value, false)}
            onKeyDown={onKeyDown}
            placeholder={t("Enter to send | Shift + Enter for newline")}
            ref={input}
            value={starting.queued ? "" : starting.draft}
          />
        </div>
      </footer>
    </section>
  );
}
