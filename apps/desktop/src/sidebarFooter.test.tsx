/**
 * The sidebar's footer: each provider's usage under its logo, and a dot on
 * the Providers button that is news only until that page has been opened.
 */
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => { throw new Error("no invoke in tests"); }), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: async () => () => undefined }) }));

import { Sidebar } from "./components/Sidebar";
import { useStore } from "./store";

const codexUpdate = (latest: string) => ({ codex: { provider: "codex", installed: "1.0.0", latest, updateAvailable: true } });

beforeEach(() => {
  localStorage.clear();
  useStore.setState({
    workspaces: [], records: {}, sessions: {}, sessionOrder: [], showHidden: false,
    view: { kind: "home" },
    usage: { codex: { primary: { usedPercent: 4, windowSeconds: 18_000 }, secondary: { usedPercent: 8, windowSeconds: 604_800 } } },
    providerVersions: codexUpdate("1.1.0"),
  } as never);
});
afterEach(cleanup);

describe("the sidebar footer", () => {
  it("names each provider's usage by its logo", () => {
    const { container } = render(<Sidebar />);
    expect(screen.getByAltText("Codex logo")).toBeTruthy();
    expect(container.querySelector(".sidebar-meter-name")?.textContent).toBe("");
  });

  it("drops the update dot once Providers has been opened, until a newer release", () => {
    const { container, rerender } = render(<Sidebar />);
    expect(container.querySelector(".badge-dot")).toBeTruthy();
    act(() => useStore.setState({ view: { kind: "signin" } } as never));
    rerender(<Sidebar />);
    act(() => useStore.setState({ view: { kind: "home" } } as never));
    rerender(<Sidebar />);
    expect(container.querySelector(".badge-dot")).toBeNull();
    act(() => useStore.setState({ providerVersions: codexUpdate("1.2.0") } as never));
    rerender(<Sidebar />);
    expect(container.querySelector(".badge-dot")).toBeTruthy();
  });
});
