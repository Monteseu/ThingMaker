/**
 * Sessions left alone for three days stop; ones that are opened, working or
 * moving keep going.
 */
import { describe, expect, it } from "vitest";
import { IDLE_STOP_AFTER_MS, idleSweep } from "./idleSessions";

const DAY = 24 * 60 * 60 * 1000;
const idle = (id: string, sequence = "5") => ({ id, sequence, busy: false });

describe("idle sessions", () => {
  it("start their clock when first seen", () => {
    const { touches, stop } = idleSweep([idle("a")], {}, 1000, null);
    expect(stop).toEqual([]);
    expect(touches.a).toEqual({ at: 1000, sequence: "5" });
  });

  it("stop after three days with nothing opened or happening", () => {
    const before = { a: { at: 0, sequence: "5" }, b: { at: 2 * DAY, sequence: "5" } };
    const { stop } = idleSweep([idle("a"), idle("b")], before, IDLE_STOP_AFTER_MS, null);
    expect(stop).toEqual(["a"]);
  });

  it("keep going while on screen, busy, or when something happened in them", () => {
    const old = { at: 0, sequence: "5" };
    const now = 4 * DAY;
    expect(idleSweep([idle("a")], { a: old }, now, "a").stop).toEqual([]);
    expect(idleSweep([{ id: "a", sequence: "5", busy: true }], { a: old }, now, null).stop).toEqual([]);
    const moved = idleSweep([idle("a", "9")], { a: old }, now, null);
    expect(moved.stop).toEqual([]);
    expect(moved.touches.a).toEqual({ at: now, sequence: "9" });
  });

  it("forget sessions that are no longer live", () => {
    expect(idleSweep([], { gone: { at: 0, sequence: "1" } }, DAY, null).touches).toEqual({});
  });
});
