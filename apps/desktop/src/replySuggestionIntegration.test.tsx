import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { NotificationSettings, Snapshot } from "@thingmaker/contracts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { api } from "./ipc";
import { EMPTY_SESSION_USAGE, useStore, type LiveSession } from "./store";
import { emptyProjection, type Card } from "./projection";
import { SessionPanel } from "./components/SessionPanel";

globalThis.ResizeObserver ??= class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });
const message = (role: "user" | "agent", text: string): Card => ({ kind: "message", key: text, message: { key: text, messageId: text, role, blocks: [{ type: "text", text }], contentUnknown: false } });

it("updates the real composer after in-place transcript updates; Tab fills its draft without sending", async () => {
  vi.useFakeTimers();
  const suggest = vi.spyOn(api, "replySuggest").mockResolvedValue("Añade las pruebas.");
  const send = vi.fn();
  const handle = { id: "suggestion-session", attachmentGeneration: "1" };
  const projection = emptyProjection();
  projection.process = "ready";
  projection.attachment = "attached";
  projection.foreground = "running";
  projection.cards = [message("agent", "Anterior"), message("user", "Mi petición")];
  const session: LiveSession = { handle, workspaceId: "w", snapshot: { handle, provider: "claude" } as Snapshot, projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ sessions: { [handle.id]: session }, settings: { replySuggestionModel: "haiku" } as NotificationSettings, drafts: {}, attachments: {}, mentions: {}, sessionTab: "transcript", send });
  render(<SessionPanel sessionId={handle.id} />);
  await act(() => vi.advanceTimersByTimeAsync(350));
  expect(suggest).not.toHaveBeenCalled();
  act(() => {
    projection.cards.push(message("agent", "Última respuesta"));
    projection.lastSequence = "3";
    projection.foreground = "succeeded";
    useStore.setState({ sessions: { [handle.id]: { ...session, projection: { ...projection } } } });
  });
  await act(() => vi.advanceTimersByTimeAsync(350));
  expect(suggest).toHaveBeenCalledWith(["Anterior", "Mi petición", "Última respuesta"]);
  const composer = screen.getByRole("textbox", { name: "Prompt" }) as HTMLTextAreaElement;
  expect(composer.placeholder).toBe("Añade las pruebas.");
  expect(composer.value).toBe("");
  fireEvent.keyDown(composer, { key: "Tab" });
  expect(useStore.getState().drafts[handle.id]).toBe("Añade las pruebas.");
  expect(composer.value).toBe("Añade las pruebas.");
  expect(send).not.toHaveBeenCalled();
});

it("never sends a Local Models session's messages to a cloud provider for a suggestion", async () => {
  vi.useFakeTimers();
  const suggest = vi.spyOn(api, "replySuggest").mockResolvedValue("Sí.");
  const handle = { id: "local-session", attachmentGeneration: "1" };
  const projection = emptyProjection();
  projection.process = "ready";
  projection.attachment = "attached";
  projection.foreground = "succeeded";
  projection.cards = [message("agent", "Anterior"), message("user", "Algo privado"), message("agent", "Respuesta")];
  const session: LiveSession = { handle, workspaceId: "w", snapshot: { handle, provider: "local" } as Snapshot, projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ sessions: { [handle.id]: session }, settings: { replySuggestionModel: "haiku", replySuggestionProvider: "claude" } as NotificationSettings, drafts: {}, attachments: {}, mentions: {}, sessionTab: "transcript" });
  render(<SessionPanel sessionId={handle.id} />);
  await act(() => vi.advanceTimersByTimeAsync(350));
  expect(suggest).not.toHaveBeenCalled();
});
