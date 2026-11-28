import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PROVIDER_LABELS, type QuotaSnapshot } from "@thingmaker/contracts";

vi.mock("./ipc", () => ({ api: {} }));
import { useStore, usageFromQuota } from "./store";
import { useLanguageForTests } from "./i18n";
import { ProviderAccounts } from "./components/ProviderAccounts";
import { ConfigPickers } from "./components/ConfigPickers";

beforeEach(() => {
  useLanguageForTests("en");
  useStore.setState({
    accounts: {}, usage: {}, quotas: {}, signIn: null, sessions: {}, pendingProvider: {},
    loadAccounts: vi.fn(async () => undefined),
  });
});
afterEach(() => { cleanup(); useLanguageForTests("en"); });

it.each(["en", "es"] as const)("renders Free Copilot meters with reset details and account guidance (%s)", (language) => {
  useLanguageForTests(language);
  const quota: QuotaSnapshot = { provider: "copilot", status: "allowed", observedAtUnixMs: 42,
    windows: [{ kind: "chat", usedPercent: 25, resetsAt: 1_900_000_000 }, { kind: "completions", usedPercent: 0 }],
  };
  useStore.setState({ quotas: { copilot: quota }, usage: { copilot: usageFromQuota(quota)! } });
  const { container } = render(<ProviderAccounts provider="copilot" />);
  expect(screen.getByText(language === "en" ? /Chat messages/ : /Mensajes de chat/).textContent).toContain("75%");
  expect(screen.getByText(language === "en" ? /Code completions/ : /Sugerencias de código/).textContent).toContain("100%");
  expect(container.querySelector(".provider-meter")?.getAttribute("title")).toContain(language === "en" ? "Reset scheduled" : "Reinicio");
  expect(screen.getByText(/copilot login/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "+ Add account" })).toBeNull();
});

it("keeps unlimited buckets alongside measured buckets", () => {
  const quota: QuotaSnapshot = { provider: "copilot", status: "allowed", observedAtUnixMs: 42,
    windows: [{ kind: "premium_interactions", usedPercent: 90 }, { kind: "chat_unlimited" }, { kind: "completions_unlimited" }],
  };
  useStore.setState({ quotas: { copilot: quota }, usage: { copilot: usageFromQuota(quota)! } });
  const { container } = render(<ProviderAccounts provider="copilot" />);
  expect(container.querySelectorAll(".provider-meter")).toHaveLength(1);
  expect(screen.getByText(/Premium interactions/).textContent).toContain("10%");
  expect(screen.getByText(/Unlimited chat messages/)).toBeTruthy();
  expect(screen.getByText(/Unlimited code completions/)).toBeTruthy();
});

it("shows guidance alone while the quota probe has no data", () => {
  const { container } = render(<ProviderAccounts provider="copilot" />);
  expect(screen.getByText(/copilot login/)).toBeTruthy();
  expect(container.querySelectorAll(".provider-meter")).toHaveLength(0);
  expect(screen.queryByRole("alert")).toBeNull();
});

it.each(["en", "es"] as const)("identifies the active Copilot Auto model and preserves the ACP value (%s)", (language) => {
  useLanguageForTests(language);
  const setConfigOption = vi.fn(async () => undefined);
  useStore.setState({ sessions: { s: { snapshot: { provider: "copilot" }, projection: { cards: [] } } } as never, setConfigOption });
  render(<ConfigPickers sessionId="s" disabled={false} configOptions={[
    { configId: "model", name: "Model", currentValue: "auto", options: [{ value: "auto", name: "Auto" }] },
  ]} />);
  const chip = screen.getByRole("button", { name: `${PROVIDER_LABELS.copilot} Auto` });
  fireEvent.click(chip);
  expect(screen.getByRole("option", { name: "Auto" }).getAttribute("aria-selected")).toBe("true");
  expect(setConfigOption).not.toHaveBeenCalled();
});
