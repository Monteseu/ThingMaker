/**
 * How full the session's context window is, as a small ring beside the model
 * picker. Opening it itemizes what the agent's own figures support: what the
 * last request sent (instructions, tools and the conversation), the reply, and
 * the free space. A split that does not add up to the agent's "used" figure is
 * left out, not shown.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { UsageCall } from "@thingmaker/contracts";
import { useStore } from "../store";
import { api } from "../ipc";
import { t } from "../i18n";

/** 958, 33k, 224.1k, 1M: the figures Claude Code's own /context uses. */
export function compactTokens(n: number): string {
  if (n < 1000) return String(n);
  const [value, unit] = n < 1_000_000 ? [n / 1000, "k"] : [n / 1_000_000, "M"];
  return `${Math.round(value * 10) / 10}${unit}`;
}

export type ContextPart = { key: "sent" | "reply"; tokens: number };

/**
 * The last request split into what it sent and the reply, when the two add up
 * to `used` (within 3%). Anything else — no transcript, a subagent's call last
 * in the file, a figure that drifted — gives nothing. (The prompt cache's
 * split is about cost, not content: after it expires the whole context reads
 * as new, so it is not used here.)
 */
export function splitLastCall(calls: UsageCall[], used: number): ContextPart[] | null {
  const last = calls.at(-1);
  if (!last || used <= 0) return null;
  const parts: ContextPart[] = [
    { key: "sent", tokens: last.inputTokens },
    { key: "reply", tokens: last.outputTokens },
  ];
  const total = parts.reduce((sum, part) => sum + part.tokens, 0);
  if (Math.abs(total - used) > used * 0.03) return null;
  return parts.filter((part) => part.tokens > 0);
}

const PART_LABELS: Record<ContextPart["key"], () => string> = {
  sent: () => t("Instructions, tools and conversation"),
  reply: () => t("Last reply"),
};

const PART_TITLES: Record<ContextPart["key"], () => string> = {
  sent: () => t("Everything the agent's last request carried: its instructions, tool definitions, files it read and the conversation so far"),
  reply: () => t("What the agent wrote in its last reply, thinking included"),
};

function Ring({ fraction, level }: { fraction: number; level: string }) {
  const radius = 6.5;
  const length = 2 * Math.PI * radius;
  const shown = Math.min(1, Math.max(fraction, fraction > 0 ? 0.03 : 0));
  return (
    <svg aria-hidden="true" className={`context-ring-svg context-${level}`} height="17" viewBox="0 0 17 17" width="17">
      <circle className="context-ring-track" cx="8.5" cy="8.5" fill="none" r={radius} strokeWidth="2.25" />
      <circle
        className="context-ring-fill"
        cx="8.5"
        cy="8.5"
        fill="none"
        r={radius}
        strokeDasharray={`${shown * length} ${length}`}
        strokeLinecap="round"
        strokeWidth="2.25"
        transform="rotate(-90 8.5 8.5)"
      />
    </svg>
  );
}

export function ContextRing({ sessionId }: { sessionId: string }) {
  const usage = useStore((s) => s.sessions[sessionId]?.projection.usage ?? null);
  const provider = useStore((s) => s.sessions[sessionId]?.snapshot.provider);
  const workspaceId = useStore((s) => s.sessions[sessionId]?.workspaceId);
  const agentSessionId = useStore((s) => s.sessions[sessionId]?.snapshot.agentSessionId ?? null);
  const [open, setOpen] = useState(false);
  const [calls, setCalls] = useState<UsageCall[] | null>(null);
  const root = useRef<HTMLSpanElement>(null);

  const used = usage?.used ?? null;
  const size = usage?.size ?? null;

  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  // The split comes from the transcript, read when the ring is opened and
  // again as the figure moves while it is open.
  useEffect(() => {
    if (!open || !workspaceId || !agentSessionId || (provider !== "claude" && provider !== "codex")) return;
    let live = true;
    api
      .sessionTokenUsage(workspaceId, agentSessionId)
      .then((read) => live && setCalls(read.calls))
      .catch(() => live && setCalls(null));
    return () => {
      live = false;
    };
  }, [open, workspaceId, agentSessionId, provider, used]);

  const parts = useMemo(() => (calls && used ? splitLastCall(calls, used) : null), [calls, used]);

  // Local Models count only the prompt Ollama did not have cached, so their
  // figure is not the window's fill; and with no size there is no ring.
  if (provider === "local" || used === null || !size || size <= 0) return null;
  const fraction = Math.min(1, used / size);
  const percent = Math.round(fraction * 100);
  const level = fraction >= 0.9 ? "full" : fraction >= 0.75 ? "high" : "ok";
  const free = Math.max(0, size - used);
  const share = (tokens: number) => `${((tokens / size) * 100).toFixed(1)}%`;
  const rows = [...(parts ?? [{ key: "used" as const, tokens: used }]), { key: "free" as const, tokens: free }];

  return (
    <span className="model-menu-anchor" ref={root}>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={t("Context: {percent}% used", { percent })}
        className="context-ring"
        onClick={() => setOpen(!open)}
        title={t("Context: {used} of {size} ({percent}%)", { used: compactTokens(used), size: compactTokens(size), percent })}
        type="button"
      >
        <Ring fraction={fraction} level={level} />
      </button>
      {open && (
        <span aria-label={t("Context")} className="model-menu context-panel" role="dialog">
          <span className="context-head">
            <strong>{t("{percent}% used", { percent })}</strong>
            <span className="muted">
              {compactTokens(used)} / {compactTokens(size)}
            </span>
          </span>
          <span aria-hidden="true" className="context-bar">
            {rows
              .filter((row) => row.key !== "free")
              .map((row) => (
                <span className={`context-seg context-seg-${row.key}`} key={row.key} style={{ width: `${(row.tokens / size) * 100}%` }} />
              ))}
          </span>
          <span className="context-rows">
            {rows.map((row) => (
              <span className="context-row" key={row.key} title={row.key === "free" ? undefined : row.key === "used" ? undefined : PART_TITLES[row.key]()}>
                <span className={`context-swatch context-seg-${row.key}`} />
                <span className="context-row-name">{row.key === "free" ? t("Free space") : row.key === "used" ? t("Used") : PART_LABELS[row.key]()}</span>
                <span className="context-row-tokens">{compactTokens(row.tokens)}</span>
                <span className="context-row-share">{share(row.tokens)}</span>
              </span>
            ))}
          </span>
        </span>
      )}
    </span>
  );
}
