/**
 * One subscription window as a short bar of what is left, with the percentage
 * and the window's name. The Providers page and the sidebar footer share it.
 */
import type { UsageWindow } from "@thingmaker/contracts";
import { t } from "../i18n";
import { windowName } from "./UsageLine";

export function UsageMeter({ usage, shortName }: { usage: UsageWindow; shortName?: string }) {
  const left = Math.max(0, 100 - Math.round(usage.usedPercent));
  const name = usage.label ?? windowName(usage.windowSeconds);
  return (
    <span className="provider-meter" title={t("{percent}% left in the {window} window, as the provider last reported it", { percent: left, window: name })}>
      <span className="provider-meter-bar">
        <span className={left < 25 ? "low" : ""} style={{ width: `${left}%` }} />
      </span>
      <b>{left}%</b> {shortName ?? name}
    </span>
  );
}

/** One row of the sidebar footer: a bucket's 5h window on the left and its 7d window on the right. */
export type WindowPair = { group?: string; short?: UsageWindow; long?: UsageWindow };

/**
 * Labelled windows (Antigravity's "Gemini · 5h", "Claude/GPT · 7d", ...)
 * paired by bucket, so each bucket reads as one row like Claude's and Codex's
 * 5h/7d pair. A label with no 5h/7d suffix gets a row of its own.
 */
export function pairWindows(windows: UsageWindow[]): WindowPair[] {
  const pairs: WindowPair[] = [];
  const byGroup = new Map<string, WindowPair>();
  for (const window of windows) {
    const match = window.label?.match(/^(.*?)(?: · |-)(5h|7d)$/);
    if (!match) {
      pairs.push({ short: window });
      continue;
    }
    const [, group, span] = match as unknown as [string, string, "5h" | "7d"];
    let pair = byGroup.get(group);
    if (!pair || pair[span === "5h" ? "short" : "long"]) {
      pair = { group };
      byGroup.set(group, pair);
      pairs.push(pair);
    }
    pair[span === "5h" ? "short" : "long"] = window;
  }
  return pairs;
}

/**
 * When a usage window refills, in Unix ms: the provider's own reset time, or
 * its "resets after" counted from when the reading was taken. Null when the
 * provider gave neither.
 */
export function resetTime(window: UsageWindow, fetchedAtUnixMs: number): number | null {
  if (window.resetAtUnix !== undefined) return window.resetAtUnix * 1000;
  if (window.resetAfterSeconds !== undefined) return fetchedAtUnixMs + window.resetAfterSeconds * 1000;
  return null;
}

/** "2h 14m", "3d 4h", "12m", "under a minute". */
export function untilText(ms: number): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  if (minutes < 1) return t("under a minute");
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const rest = minutes % 60;
  if (days > 0) return hours > 0 ? t("{days}d {hours}h", { days, hours }) : t("{days}d", { days });
  if (hours > 0) return rest > 0 ? t("{hours}h {minutes}m", { hours, minutes: rest }) : t("{hours}h", { hours });
  return t("{minutes}m", { minutes: rest });
}
