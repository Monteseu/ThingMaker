import { afterEach, beforeEach, expect, it, vi } from "vitest";

const prefGet = vi.hoisted(() => vi.fn());
const prefSet = vi.hoisted(() => vi.fn(async () => undefined));

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("./ipc", () => ({ api: { prefGet, prefSet } }));

import { normalizeTheme, useStore } from "./store";

// jsdom ships no matchMedia, so install a controllable stub for the tests that
// exercise the "system" migration and restore it afterwards.
function stubMatchMedia(prefersLight: boolean) {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: vi.fn(() => ({ matches: prefersLight }) as MediaQueryList),
  });
}

beforeEach(() => {
  prefGet.mockReset();
  prefSet.mockClear();
});

afterEach(() => {
  delete (window as { matchMedia?: unknown }).matchMedia;
});

it("keeps the three named themes untouched", () => {
  expect(normalizeTheme("dark")).toBe("dark");
  expect(normalizeTheme("gray")).toBe("gray");
  expect(normalizeTheme("light")).toBe("light");
});

it("maps a dropped 'system' theme to the look it was showing", () => {
  stubMatchMedia(true);
  expect(normalizeTheme("system")).toBe("light");
  stubMatchMedia(false);
  expect(normalizeTheme("system")).toBe("dark");
});

it("falls back to dark for absent or unknown themes", () => {
  expect(normalizeTheme(undefined)).toBe("dark");
  expect(normalizeTheme("neon")).toBe("dark");
});

it("migrates a stored 'system' theme on load and rewrites the save", async () => {
  stubMatchMedia(false);
  prefGet.mockImplementation(async (key: string) => (key === "ui_prefs" ? { theme: "system", enterSends: false } : undefined));

  await useStore.getState().loadUiPrefs();

  expect(useStore.getState().uiPrefs.theme).toBe("dark");
  expect(useStore.getState().uiPrefs.enterSends).toBe(false);
  expect(prefSet).toHaveBeenCalledWith("ui_prefs", expect.objectContaining({ theme: "dark", enterSends: false }));
});

it("leaves a valid stored theme alone without rewriting", async () => {
  prefGet.mockImplementation(async (key: string) => (key === "ui_prefs" ? { theme: "light" } : undefined));

  await useStore.getState().loadUiPrefs();

  expect(useStore.getState().uiPrefs.theme).toBe("light");
  expect(prefSet).not.toHaveBeenCalled();
});
