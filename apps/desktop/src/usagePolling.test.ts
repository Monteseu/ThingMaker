import { expect, it, vi } from "vitest";

const nativeWindow = vi.hoisted(() => ({
  isFocused: vi.fn(async () => false),
  onFocusChanged: vi.fn(async (_handler: (event: { payload: boolean }) => void) => () => undefined),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => nativeWindow }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("./ipc", () => ({
  api: {
    executionProfiles: vi.fn(async () => []),
    workspaceList: vi.fn(async () => []),
  },
}));

import { useStore } from "./store";

it("polls Windows account usage only while focused and catches up on return", async () => {
  const initial = useStore.getState();
  vi.useFakeTimers();
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows");
  // An initially hidden WebView2 can claim focus before it receives a blur.
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  const refreshUsage = vi.fn(async () => null);
  const idle = async () => undefined;
  useStore.setState({
    refreshUsage,
    loadSettings: idle,
    loadUiPrefs: idle,
    loadContinuations: idle,
    loadProviders: idle,
    loadProviderVersions: idle,
    loadDefaultCombo: idle,
    loadTeamPresets: idle,
    loadQuickChats: idle,
    checkForUpdate: idle,
    rehydrate: idle,
  });

  try {
    await useStore.getState().bootstrap();
    const reportFocus = nativeWindow.onFocusChanged.mock.calls[0]?.[0];
    if (!reportFocus) throw new Error("Missing native focus listener");
    vi.advanceTimersByTime(20 * 60_000);
    expect(refreshUsage).not.toHaveBeenCalled();

    reportFocus({ payload: true });
    expect(refreshUsage.mock.calls).toEqual([["claude"], ["codex"], ["gemini"]]);

    reportFocus({ payload: false });
    vi.advanceTimersByTime(20 * 60_000);
    expect(refreshUsage).toHaveBeenCalledTimes(3);

    reportFocus({ payload: true });
    reportFocus({ payload: true });
    expect(refreshUsage).toHaveBeenCalledTimes(6);
    vi.advanceTimersByTime(5 * 60_000);
    expect(refreshUsage).toHaveBeenCalledTimes(9);
  } finally {
    vi.useRealTimers();
    vi.restoreAllMocks();
    useStore.setState(initial, true);
  }
});
