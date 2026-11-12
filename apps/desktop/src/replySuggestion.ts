import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Card } from "./projection";
import { api } from "./ipc";

/** Last reply on either side of the last user turn; never thoughts or tools. */
export function suggestionMessages(cards: readonly Card[]): [string, string, string] | null {
  const messages = cards.filter((card): card is Extract<Card, { kind: "message" }> => card.kind === "message" && card.message.role !== "thought");
  const latest = messages.at(-1);
  if (!latest || latest.message.role !== "agent") return null;
  const userIndex = messages.map((card) => card.message.role).lastIndexOf("user");
  if (userIndex < 1) return null;
  const previous = messages.slice(0, userIndex).reverse().find((card) => card.message.role === "agent");
  const user = messages[userIndex];
  if (!previous || !user) return null;
  const texts = [previous, user, latest].map((card) => card.message.blocks.filter((block) => block.type === "text").map((block) => block.text).join("").trim());
  if (texts.some((text) => !text || new TextEncoder().encode(text).length > 64 * 1024)) return null;
  return texts as [string, string, string];
}

export function useReplySuggestion(sessionId: string, model: string | undefined, messages: [string, string, string] | null, eligible: boolean, draft: string) {
  const key = JSON.stringify([sessionId, model, messages]);
  const [result, setResult] = useState<{ key: string; text: string } | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const attempted = useRef<string | null>(null);
  const hasMessages = messages !== null;
  useEffect(() => {
    if (draft) setDismissed(key);
    if (!eligible) setResult(null);
    if (!eligible || draft || !model || !hasMessages || dismissed === key) return;
    let cancelled = false;
    // Wait for the finished turn and any queued prompt to settle.
    const timer = window.setTimeout(() => {
      if (attempted.current === key) return;
      attempted.current = key;
      const context = (JSON.parse(key) as [string, string, [string, string, string]])[2];
      void api.replySuggest(context).then((text) => {
        if (!cancelled) setResult({ key, text });
      }).catch(() => { /* Optional enhancement: generation failure leaves the composer usable. */ });
    }, 300);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [key, eligible, draft, model, hasMessages, dismissed]);
  const dismiss = () => setDismissed(key);
  return { suggestion: eligible && !draft && dismissed !== key && result?.key === key ? result.text : "", dismiss };
}

/** Consumes only plain Tab or Escape, never sending a message. */
export function suggestionKey(event: KeyboardEvent<HTMLTextAreaElement>, suggestion: string, accept: (text: string) => void, dismiss: () => void): boolean {
  if (!suggestion || event.nativeEvent.isComposing || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return false;
  if (event.key !== "Tab" && event.key !== "Escape") return false;
  event.preventDefault();
  if (event.key === "Tab") accept(suggestion);
  dismiss();
  return true;
}
