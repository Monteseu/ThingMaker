import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { pairWindows, UsageMeter } from "./components/UsageMeter";

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
