import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { usageFromQuota } from "./store";
import { copilotQuotaLabel } from "./copilotQuota";
import { useLanguageForTests } from "./i18n";
import { UsageMeter } from "./components/UsageMeter";

afterEach(() => { cleanup(); useLanguageForTests("en"); });

it.each(["en", "es"] as const)("renders all applicable Copilot bars in %s", (language) => {
  useLanguageForTests(language);
  const usage = usageFromQuota({ provider: "copilot", status: "allowed", observedAtUnixMs: 42,
    windows: [
      { kind: "premium_interactions", usedPercent: 50 },
      { kind: "chat", usedPercent: 0.1 },
      { kind: "completions", usedPercent: 0 },
    ],
  })!;
  expect(usage.allowed).toBe(true);
  expect(usage.windows).toHaveLength(3);
  render(<>{usage.windows!.map((window, index) => <UsageMeter key={index} usage={window} fetchedAtUnixMs={42} />)}</>);
  for (const label of language === "en"
    ? ["Premium interactions", "Chat messages", "Code completions"]
    : ["Interacciones premium", "Mensajes de chat", "Sugerencias de código"]) {
    expect(screen.getByText(label, { exact: false })).toBeTruthy();
  }
  expect(usage.windows?.map(window => window.usedPercent)).toEqual([50, 0.1, 0]);
});

it("labels unlimited Free buckets and preserves unknown usage", () => {
  useLanguageForTests("es");
  expect(copilotQuotaLabel("chat_unlimited")).toBe("Mensajes de chat ilimitados");
  expect(copilotQuotaLabel("completions_unlimited")).toBe("Sugerencias de código ilimitadas");
  expect(usageFromQuota({ provider: "copilot", status: "allowed", observedAtUnixMs: 42,
    windows: [{ kind: "chat_unlimited" }, { kind: "completions_unlimited" }],
  })).toBeNull();
});
