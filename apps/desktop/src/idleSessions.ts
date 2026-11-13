/**
 * Sessions left alone stop on their own. A live session whose agent has sat
 * idle for three days, with nobody opening it and nothing happening in it, is
 * stopped; its conversation stays in the list and resumes like any other.
 *
 * "Something happening" counts as well as opening it here, so a session the
 * phone keeps busy is not stopped from under it.
 */

export const IDLE_STOP_AFTER_MS = 3 * 24 * 60 * 60 * 1000;
const STORAGE_KEY = "thingmaker.sessions.lastTouched";

/** When each live session was last opened or last moved, and how far it had got. */
export type Touches = Record<string, { at: number; sequence: string }>;

export type IdleCandidate = {
  id: string;
  sequence: string;
  /** A turn, a background call, a subagent or a Big Thing run is under way. */
  busy: boolean;
};

/**
 * The touches brought up to date for `now`, and which sessions to stop.
 * A session seen for the first time starts its clock now; one whose event
 * sequence moved, or that is open on screen, is touched now. Sessions no
 * longer live are dropped.
 */
export function idleSweep(candidates: IdleCandidate[], touches: Touches, now: number, onScreen: string | null): { touches: Touches; stop: string[] } {
  const next: Touches = {};
  const stop: string[] = [];
  for (const session of candidates) {
    const seen = touches[session.id];
    const touched = !seen || seen.sequence !== session.sequence || session.id === onScreen || session.busy;
    const entry = touched ? { at: now, sequence: session.sequence } : seen;
    next[session.id] = entry;
    if (!touched && now - entry.at >= IDLE_STOP_AFTER_MS) stop.push(session.id);
  }
  return { touches: next, stop };
}

/** Marks one session as opened now. */
export function touch(touches: Touches, id: string, sequence: string, now: number): Touches {
  return { ...touches, [id]: { at: now, sequence } };
}

export function loadTouches(): Touches {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === "object" ? (parsed as Touches) : {};
  } catch {
    return {};
  }
}

export function saveTouches(touches: Touches) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(touches));
  } catch {
    // Without storage the clock restarts with the app, which only delays a stop.
  }
}
