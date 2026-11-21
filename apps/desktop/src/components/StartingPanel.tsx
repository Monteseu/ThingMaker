import { VoiceInput } from "./VoiceInput";
/**
 * A session while its agent starts — a new one, or one being resumed: the
 * chat is there at once, so the person sees something happening and can
 * write. A message sent now waits
 * and goes out as soon as the session is ready; then this view gives way to
 * the session itself.
 */
import { useEffect, useRef, type KeyboardEvent } from "react";
import { PROVIDER_LABELS } from "@thingmaker/contracts";
import { useStore } from "../store";
import { ProviderMark } from "./ProviderMark";
import { t } from "../i18n";
import type { AttachmentSnapshot } from "@thingmaker/contracts";
import { AttachmentChips } from "./AttachmentChips";

const EMPTY_ATTACHMENTS: AttachmentSnapshot[] = [];

export function StartingPanel() {
  const starting = useStore((s) => s.starting);
  const setStartingDraft = useStore((s) => s.setStartingDraft);
  const workspace = useStore((s) => s.workspaces.find((entry) => entry.id === s.starting?.workspaceId));
  const enterSends = useStore((s) => s.uiPrefs.enterSends);
  const input = useRef<HTMLTextAreaElement>(null);
  const attachments = useStore((state) => starting ? state.attachments[starting.id] ?? EMPTY_ATTACHMENTS : EMPTY_ATTACHMENTS);
  const removeAttachment = useStore((state) => state.removeAttachment);

  useEffect(() => {
    input.current?.focus();
  }, []);

  if (!starting) return null;
  const provider = PROVIDER_LABELS[starting.provider];
  const heading = starting.resuming ? starting.title || t("Conversation") : t("New session");
  const queue = () => {
    if (starting.draft.trim() || attachments.length) setStartingDraft(starting.draft, true);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
    const sends = enterSends ? !event.shiftKey : event.shiftKey;
    if (!sends) return;
    event.preventDefault();
    queue();
  };

  return (
    <section aria-label={heading} className="panel session-panel starting-panel">
      <header className="session-header">
        <strong>{heading}</strong>
        {workspace && <span className="muted">{workspace.displayPath.split(/[\\/]/).pop()}</span>}
      </header>
      <div className="transcript">
        <div className="starting-status" role="status">
          <div aria-hidden="true" className={`starting-mark${starting.error ? " starting-mark-error" : ""}`}>
            <ProviderMark provider={starting.provider} size={32} />
          </div>
          {starting.error ? <p className="chip-warn-text">{starting.resuming
            ? t("{provider} could not resume this conversation: {error}", { provider, error: starting.error })
            : t("{provider} could not start: {error}", { provider, error: starting.error })}</p> : <>
            <strong>{starting.resuming ? t("Recovering the conversation…") : t("Setting up the office…")}</strong>
            <span className="muted">{starting.resuming ? t("Resuming {provider}…", { provider }) : t("Starting {provider}…", { provider })}</span>
            <p className="small muted">{t("You can write now. Your message will be sent when everything is ready.")}</p>
          </>}
        </div>
        {starting.queued && (
          <article className="card card-user">
            <div className="starting-queued">{starting.draft}</div>
            <span className="small muted">{t("Sends as soon as {provider} is ready", { provider })}</span>
          </article>
        )}
      </div>
      <footer className="composer">
        <AttachmentChips files={attachments} onRemove={(id) => removeAttachment(starting.id, id)} />
        <div className="composer-wrap">
          <textarea
            aria-label={t("Prompt")}
            className="textarea"
            disabled={starting.queued}
            onChange={(event) => setStartingDraft(event.target.value, false)}
            onKeyDown={onKeyDown}
            onDrop={(event) => {
              if (Array.from(event.dataTransfer.types ?? []).some((type) => type === "Files" || type === "text/uri-list")) event.preventDefault();
            }}
            placeholder={t("Enter to send | Shift + Enter for newline")}
            ref={input}
            value={starting.queued ? "" : starting.draft}
          />
        </div>
        <VoiceInput key={starting.workspaceId} target={starting.workspaceId} draft={starting.draft} input={input} localOnly={starting.provider === "local"} disabled={starting.queued} onChange={(text) => setStartingDraft(text, false)} />
      </footer>
    </section>
  );
}
