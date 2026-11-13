import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { pairWindows, resetTime, untilText, UsageMeter } from "./components/UsageMeter";

it("pairs each Antigravity bucket's 5h and 7d windows on one row", () => {
  const w = (label: string) => ({ label, usedPercent: 0, windowSeconds: 0 });
  const pairs = pairWindows([w("Gemini · 5h"), w("Claude/GPT · 5h"), w("Gemini · argon-wtu-7d"), w("Gemini · 7d"), w("Claude/GPT · 7d")]);
  expect(pairs.map((p) => [p.group, p.short?.label, p.long?.label])).toEqual([
    ["Gemini", "Gemini · 5h", "Gemini · 7d"],
    ["Claude/GPT", "Claude/GPT · 5h", "Claude/GPT · 7d"],
    ["Gemini · argon-wtu", undefined, "Gemini · argon-wtu-7d"],
  ]);
});

it("distinguishes Antigravity buckets that share a weekly reset window", () => {
  render(<>
    <UsageMeter usage={{ label: "Gemini · 7d", usedPercent: 25, windowSeconds: 604800 }} />
    <UsageMeter usage={{ label: "Claude/GPT · 7d", usedPercent: 100, windowSeconds: 604800 }} />
  </>);
  expect(screen.getByText("Gemini · 7d").textContent).toContain("75%");
  expect(screen.getByText("Claude/GPT · 7d").textContent).toContain("0%");
});

describe("when a spent window refills", () => {
  it("uses the provider's reset time, or its countdown from the reading", () => {
    expect(resetTime({ usedPercent: 100, windowSeconds: 18_000, resetAtUnix: 2_000 }, 0)).toBe(2_000_000);
    expect(resetTime({ usedPercent: 100, windowSeconds: 18_000, resetAfterSeconds: 60 }, 5_000)).toBe(65_000);
    expect(resetTime({ usedPercent: 100, windowSeconds: 18_000 }, 0)).toBeNull();
  });

  it("reads as a short wait", () => {
    expect(untilText(20_000)).toBe("under a minute");
    expect(untilText(12 * 60_000)).toBe("12m");
    expect(untilText((2 * 60 + 14) * 60_000)).toBe("2h 14m");
    expect(untilText((3 * 1440 + 4 * 60) * 60_000)).toBe("3d 4h");
  });
});
