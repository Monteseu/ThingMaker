import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { UsageMeter } from "./components/UsageMeter";

it("distinguishes Antigravity buckets that share a weekly reset window", () => {
  render(<>
    <UsageMeter usage={{ label: "Gemini · 7d", usedPercent: 25, windowSeconds: 604800 }} />
    <UsageMeter usage={{ label: "Claude/GPT · 7d", usedPercent: 100, windowSeconds: 604800 }} />
  </>);
  expect(screen.getByText("Gemini · 7d").textContent).toContain("75%");
  expect(screen.getByText("Claude/GPT · 7d").textContent).toContain("0%");
});
