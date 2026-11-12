import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import type { NotificationSettings, ProviderModel } from "@thingmaker/contracts";
vi.mock("./ipc", () => ({ api: { providerModels: vi.fn() } }));
import { api } from "./ipc";
import { ReplySuggestionSettings } from "./components/ReplySuggestionSettings";
const model = (id: string): ProviderModel => ({ id, name: id, needsCredits: false, efforts: [], inputImage: false, isDefault: false });
afterEach(() => { cleanup(); vi.resetAllMocks(); });

it("preserves old Claude selections, clears the model when switching provider and saves both choices", async () => {
  vi.mocked(api.providerModels).mockImplementation(async (provider) => [model(provider === "claude" ? "haiku" : "gpt-test")]);
  const changed = vi.fn();
  function Settings() {
    const [settings, setSettings] = useState({ replySuggestionModel: "haiku" } as NotificationSettings);
    return <ReplySuggestionSettings settings={settings} onChange={(patch) => { changed(patch); setSettings({ ...settings, ...patch }); }} />;
  }
  await act(async () => { render(<Settings />); });
  expect((screen.getByRole("combobox", { name: "Reply suggestion provider" }) as HTMLSelectElement).value).toBe("claude");
  expect((screen.getByRole("combobox", { name: /Reply suggestion model/ }) as HTMLSelectElement).value).toBe("haiku");
  await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Reply suggestion provider" }), { target: { value: "codex" } }); });
  expect(changed).toHaveBeenLastCalledWith({ replySuggestionProvider: "codex", replySuggestionModel: "" });
  expect(screen.queryByRole("option", { name: "haiku" })).toBeNull();
  fireEvent.change(screen.getByRole("combobox", { name: /Reply suggestion model/ }), { target: { value: "gpt-test" } });
  expect(changed).toHaveBeenLastCalledWith({ replySuggestionProvider: "codex", replySuggestionModel: "gpt-test" });
});

it("ignores the old provider's slow catalog and offers local chat models without a Claude dependency", async () => {
  let resolve!: (models: ProviderModel[]) => void;
  vi.mocked(api.providerModels).mockImplementation((provider) => provider === "claude" ? new Promise((done) => { resolve = done; }) : Promise.resolve([model("@chat/qwen"), model("@agent/qwen")]));
  const { rerender } = render(<ReplySuggestionSettings settings={{} as NotificationSettings} onChange={vi.fn()} />);
  await act(async () => { rerender(<ReplySuggestionSettings settings={{ replySuggestionProvider: "local" } as NotificationSettings} onChange={vi.fn()} />); });
  await act(async () => resolve([model("haiku")]));
  expect(screen.getByRole("option", { name: "@chat/qwen" })).toBeTruthy();
  expect(screen.queryByRole("option", { name: "@agent/qwen" })).toBeNull();
  expect(screen.queryByRole("option", { name: "haiku" })).toBeNull();
});

it("reports a provider catalog failure instead of substituting Claude models", async () => {
  vi.mocked(api.providerModels).mockRejectedValue(new Error("Sign in to Antigravity"));
  await act(async () => { render(<ReplySuggestionSettings settings={{ replySuggestionProvider: "gemini" } as NotificationSettings} onChange={vi.fn()} />); });
  expect(screen.getByRole("status").textContent).toContain("Sign in to Antigravity");
  expect(screen.getAllByRole("option")).toHaveLength(5);
});
