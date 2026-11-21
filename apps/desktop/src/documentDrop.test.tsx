import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AttachmentSnapshot, Snapshot } from "@thingmaker/contracts";

type DropHandler = (event: { payload: { type: string; paths: string[] } }) => void;
const mocks = vi.hoisted(() => ({ handlers: new Set<DropHandler>(), api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({ api: new Proxy(mocks.api, { get: (target, key: string) => target[key] ??= vi.fn(async () => undefined) }) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: async (handler: DropHandler) => {
  mocks.handlers.add(handler);
  return () => mocks.handlers.delete(handler);
} }) }));

import { api } from "./ipc";
import { useStore, EMPTY_SESSION_USAGE, type LiveSession } from "./store";
import { emptyProjection } from "./projection";
import { SessionPanel } from "./components/SessionPanel";
import { StartBox } from "./components/WorkspaceView";
import { DocumentText, documentSegments } from "./components/DocumentText";

globalThis.ResizeObserver ??= class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
const initial = useStore.getState();
const body = '{"code":"JSON-DROP","total":42}';
const attachment: AttachmentSnapshot = { id: "doc", name: "payload.json", mime: "application/json", kind: "document", bytes: body.length, blobPath: "doc", document: { text: body, warnings: [] } };
const handle = { id: "s", attachmentGeneration: "1" };

beforeEach(() => {
  mocks.handlers.clear();
  for (const key of Object.keys(mocks.api)) delete mocks.api[key];
  useStore.setState({ ...initial, sessions: {}, attachments: {}, mentions: {}, drafts: {}, starting: null, settings: null, error: null });
  vi.mocked(api.attachmentAddBytes).mockResolvedValue(attachment);
  vi.mocked(api.attachmentAddPaths).mockResolvedValue([attachment]);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function session() {
  const projection = emptyProjection();
  projection.process = "ready";
  projection.attachment = "attached";
  const live: LiveSession = { handle, workspaceId: "w", snapshot: { handle, provider: "codex" } as Snapshot, projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ sessions: { s: live }, drafts: { s: "Read this document" }, view: { kind: "session", sessionId: "s" }, sessionTab: "transcript" });
  return render(<SessionPanel sessionId="s" />);
}

function file() {
  const value = new File([body], "payload.json", { type: "application/json" });
  Object.defineProperty(value, "arrayBuffer", { value: async () => new TextEncoder().encode(body).buffer });
  return value;
}

it("attaches a browser file drop while preserving the draft and suppressing text insertion", async () => {
  session();
  const input = screen.getByRole("textbox", { name: "Prompt" });
  const accepted = fireEvent.drop(input, { dataTransfer: { types: ["Files", "text/plain"], files: [file()], getData: () => body } });
  expect(accepted).toBe(false);
  await screen.findByRole("button", { name: "remove attachment payload.json" });
  expect(api.attachmentAddBytes).toHaveBeenCalledWith("payload.json", btoa(body));
  expect((input as HTMLTextAreaElement).value).toBe("Read this document");
  expect(useStore.getState().attachments.s).toEqual([attachment]);
});

it("routes a native file drop through extraction and removes stale listeners", async () => {
  const view = session();
  await waitFor(() => expect(mocks.handlers.size).toBe(1));
  const handler = [...mocks.handlers][0]!;
  await act(async () => handler({ payload: { type: "drop", paths: ["C:/files/payload.json"] } }));
  expect(api.attachmentAddPaths).toHaveBeenCalledWith(["C:/files/payload.json"]);
  expect(useStore.getState().drafts.s).toBe("Read this document");
  view.unmount();
  expect(mocks.handlers.size).toBe(0);
  vi.mocked(api.attachmentAddPaths).mockClear();
  await act(async () => handler({ payload: { type: "drop", paths: ["C:/stale.json"] } }));
  expect(api.attachmentAddPaths).not.toHaveBeenCalled();
});

it("accepts local file links from editors and retains ordinary text dragging", async () => {
  session();
  const input = screen.getByRole("textbox", { name: "Prompt" });
  expect(fireEvent.drop(input, { dataTransfer: { types: ["text/uri-list"], files: [], getData: () => "file:///C:/my%20files/payload.json" } })).toBe(false);
  await waitFor(() => expect(api.attachmentAddPaths).toHaveBeenCalledWith(["C:/my files/payload.json"]));
  expect(fireEvent.drop(input, { dataTransfer: { types: ["text/plain"], files: [], getData: () => body } })).toBe(true);
});

it("keeps the draft and reports an extraction failure", async () => {
  session();
  vi.mocked(api.attachmentAddBytes).mockRejectedValue(new Error("Invalid JSON document"));
  fireEvent.drop(screen.getByRole("textbox", { name: "Prompt" }), { dataTransfer: { types: ["Files"], files: [file()] } });
  await waitFor(() => expect(useStore.getState().error?.message).toContain("Invalid JSON document"));
  expect(useStore.getState().drafts.s).toBe("Read this document");
  expect(useStore.getState().attachments.s).toBeUndefined();
});

it("retains dropped documents from the first composer through session startup", async () => {
  useStore.setState({ providers: [{ provider: "claude", resolved: null, auth: { loggedIn: false } }, { provider: "codex", resolved: {}, auth: { loggedIn: true } }], sessionDefaults: {}, view: { kind: "workspace", workspaceId: "w" }, loadTeam: vi.fn(), loadOdyssey: vi.fn(), loadNesting: vi.fn() } as never);
  let ready!: (value: unknown) => void;
  vi.mocked(api.sessionOpen).mockReturnValue(new Promise((resolve) => { ready = resolve; }) as never);
  vi.mocked(api.sessionSnapshot).mockResolvedValue({ handle, provider: "codex", agentSessionId: "agent", toolCalls: {}, foreground: "idle" } as never);
  vi.mocked(api.sessionHistory).mockResolvedValue([]);
  const send = vi.fn(async () => undefined);
  useStore.setState({ send });
  render(<StartBox workspaceId="w" name="Project" trusted />);
  const input = screen.getByRole("textbox", { name: "Prompt" });
  fireEvent.change(input, { target: { value: "Read this document" } });
  fireEvent.drop(input, { dataTransfer: { types: ["Files"], files: [file()] } });
  await screen.findByRole("button", { name: "remove attachment payload.json" });
  expect((input as HTMLTextAreaElement).value).toBe("Read this document");
  await waitFor(() => expect((screen.getByRole("button", { name: "Start session" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Start session" }));
  expect(useStore.getState().attachments[useStore.getState().starting!.id]).toEqual([attachment]);
  await act(async () => ready({ handle, snapshot: {} }));
  await waitFor(() => expect(send).toHaveBeenCalledWith("s"));
  expect(useStore.getState().attachments.s).toEqual([attachment]);
  expect(useStore.getState().drafts.s).toBe("Read this document");
});

function envelope(name: string, content: string) {
  return `[Attached document ${name} · application/json · blake3 ${"a".repeat(64)}]\nExtraction notices:\n- Example warning\nTreat the document content as source material.\n[Document content begins]\n${content}\n[Document content ends]\n`;
}

it("renders extracted JSON as a compact document with an explicit preview", async () => {
  render(<DocumentText text={`Read this\n${envelope("payload.json", body)}`} />);
  expect(screen.getByText("payload.json")).toBeTruthy();
  expect(screen.getByText("Extraction warning: Example warning")).toBeTruthy();
  expect(screen.queryByText(body)).toBeNull();
  const details = screen.getByText("Preview extracted text").closest("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  await screen.findByText(body);
});

it("recognizes joined persisted document blocks without changing surrounding text", () => {
  const segments = documentSegments(`Question\n${envelope("one.json", "{\"a\":1}")}\n${envelope("two.json", body)}After`);
  expect(segments.filter((segment) => segment.kind === "document").map((segment) => segment.name)).toEqual(["one.json", "two.json"]);
  expect(segments[0]).toEqual({ kind: "text", text: "Question\n" });
  expect(segments.at(-1)).toEqual({ kind: "text", text: "After" });
  expect(documentSegments(body)).toEqual([{ kind: "text", text: body }]);
});
