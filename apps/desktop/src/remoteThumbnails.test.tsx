/**
 * Pictures in tool output reach the phone as thumbnails; a tap fetches the
 * original from the computer.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ToolPatch } from "@thingmaker/contracts";

const calls: { command: string; args: Record<string, unknown> }[] = [];
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "remote_image" && args.id === "gone") throw { code: "NOT_FOUND", message: "That picture is no longer kept on the computer.", retry: "never" };
    if (command === "remote_image") return { mimeType: "image/png", data: "ORIGINAL" };
    return null;
  }),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { ToolPayloads } from "./components/ToolPayloads";
import { toolImages } from "./toolImages";

const thumb = "THUMB";
const block = (id: string) => ({ type: "image", data: thumb, mimeType: "image/jpeg", thingmakerFull: { id, bytes: 640 * 1024, mimeType: "image/png" } });
const patch = (content: unknown): ToolPatch => ({ toolCallId: "shot", title: "Screenshot", status: "completed", toolKind: "other", content, rawInput: null, rawOutput: null, name: null, locations: null, meta: null, present: [], cleared: [] } as ToolPatch);

afterEach(() => {
  cleanup();
  calls.length = 0;
});

describe("thumbnails on the phone", () => {
  it("keeps the way to the original with the picture", () => {
    expect(toolImages([{ type: "content", content: block("abc") }]).images).toEqual([{ full: { id: "abc", bytes: 640 * 1024, mimeType: "image/png" }, data: thumb, mimeType: "image/jpeg" }]);
  });

  it("shows the thumbnail and fetches the original only when tapped", async () => {
    render(<ToolPayloads patch={patch([{ type: "content", content: block("abc") }])} workspaceId="w" />);
    expect(screen.getByRole("img").getAttribute("src")).toBe(`data:image/jpeg;base64,${thumb}`);
    expect(screen.getByText("Preview · tap for full size (640 KiB)")).toBeTruthy();
    expect(calls.filter((call) => call.command === "remote_image")).toHaveLength(0);
    fireEvent.click(screen.getByTitle("Tap to load the full picture"));
    await waitFor(() => expect(screen.getAllByRole("img").some((img) => img.getAttribute("src") === "data:image/png;base64,ORIGINAL")).toBe(true));
    expect(calls.filter((call) => call.command === "remote_image")).toEqual([{ command: "remote_image", args: { id: "abc" } }]);
  });

  it("says so when the computer no longer has the original", async () => {
    render(<ToolPayloads patch={patch([{ type: "content", content: block("gone") }])} workspaceId="w" />);
    fireEvent.click(screen.getByTitle("Tap to load the full picture"));
    await waitFor(() => expect(screen.getByText("That picture is no longer kept on the computer.")).toBeTruthy());
  });
});
