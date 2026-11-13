/**
 * Settings in tabs: the tab comes from the view (⌘I opens Integrations),
 * reply suggestions are one menu over every ready provider's models, and a
 * default model chosen here is what new sessions start with.
 */
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderModel } from "@thingmaker/contracts";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { useStore } from "./store";
import { SettingsPanel } from "./components/SettingsPanel";

const model = (id: string, efforts: string[] = []): ProviderModel => ({ id, name: id, needsCredits: false, efforts, inputImage: false, isDefault: false });
const settings = {
  enabled: true,
  notifyCompleted: true,
  notifyFailed: true,
  notifyNeedsInput: true,
  quietHoursStart: "00:00",
  quietHoursEnd: "00:00",
  mutedWorkspaceIds: [],
  closeBehavior: "ask",
  keepAwake: true,
  keepAwakeOnBattery: false,
  replySuggestionModel: "",
};

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({
    settings,
    providers: [
      { provider: "claude", label: "Claude Code", resolved: {}, auth: { loggedIn: true } },
      { provider: "codex", label: "Codex", resolved: {}, auth: { loggedIn: true } },
    ],
    providerModels: { claude: [model("opus", ["low", "high"]), model("haiku")], codex: [model("gpt-test")] },
    sessionDefaults: {},
    workspaces: [],
    view: { kind: "settings" },
  } as never);
});
afterEach(cleanup);

describe("Settings", () => {
  it("opens on the tab the view asks for, and switches from the bar", () => {
    useStore.setState({ view: { kind: "settings", tab: "chat" } } as never);
    render(<SettingsPanel />);
    expect(screen.getByRole("tab", { name: "Chat" }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(screen.getByRole("tab", { name: "Default Models" }));
    expect(useStore.getState().view).toEqual({ kind: "settings", tab: "models" });
  });

  it("offers reply suggestions from every ready provider in one menu", async () => {
    const saveSettings = vi.fn(async () => undefined);
    useStore.setState({ view: { kind: "settings", tab: "chat" }, saveSettings } as never);
    render(<SettingsPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Reply suggestions" }));
    fireEvent.click(screen.getByRole("option", { name: "Codex · gpt-test" }));
    expect(saveSettings).toHaveBeenCalledWith(expect.objectContaining({ replySuggestionProvider: "codex", replySuggestionModel: "gpt-test" }));
  });

  it("sets the model and effort new sessions start with", async () => {
    calls.api.prefSet = vi.fn(async () => undefined);
    useStore.setState({ view: { kind: "settings", tab: "models" } } as never);
    render(<SettingsPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Claude Code model" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("option", { name: "haiku" }));
    });
    expect(useStore.getState().sessionDefaults.claude).toEqual({ model: "haiku" });
    expect(calls.api.prefSet).toHaveBeenCalledWith("session_defaults", { claude: { model: "haiku" } });
  });

  it("shows the alert types only while notifications are on", () => {
    useStore.setState({ view: { kind: "settings", tab: "notifications" } } as never);
    const { rerender } = render(<SettingsPanel />);
    expect(screen.getByRole("switch", { name: "A turn finished" })).toBeTruthy();
    useStore.setState({ settings: { ...settings, enabled: false } } as never);
    rerender(<SettingsPanel />);
    expect(screen.queryByRole("switch", { name: "A turn finished" })).toBeNull();
  });
});
