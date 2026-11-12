/**
 * The transcript shows the images agents make and look at, once each, where
 * they first appear — found by path, since neither provider sends an image
 * block for them.
 */
import { describe, expect, it } from "vitest";
import type { ToolPatch } from "@thingmaker/contracts";
import type { Card } from "./projection";
import { assignImages, foldersIn, imageRefFor, imageRefsIn, textOf } from "./transcriptImages";

const ROOT = "/Users/me/Project";
const GENERATED = "/Users/me/.codex/generated_images/01a0f675/exec-bd6f.png";

describe("finding images by path", () => {
  it("reads workspace paths, absolute or relative, and Codex's generated images", () => {
    expect(imageRefFor(`${ROOT}/assets/logo.png`, ROOT)).toEqual({ kind: "workspace", relative: "assets/logo.png" });
    expect(imageRefFor("./assets/logo.png", ROOT)).toEqual({ kind: "workspace", relative: "assets/logo.png" });
    expect(imageRefFor(GENERATED, ROOT)).toEqual({ kind: "generated", path: GENERATED });
    expect(imageRefFor(`file://${ROOT}/a.jpg`, ROOT)).toEqual({ kind: "workspace", relative: "a.jpg" });
  });

  it("never reaches outside the workspace or Codex's image folder", () => {
    expect(imageRefFor("/etc/secret.png", ROOT)).toBeNull();
    expect(imageRefFor("../elsewhere/x.png", ROOT)).toBeNull();
    expect(imageRefFor("https://example.com/x.png", ROOT)).toBeNull();
    expect(imageRefFor(`${ROOT}/notes.md`, ROOT)).toBeNull();
    expect(imageRefFor(`${ROOT}-other/x.png`, ROOT)).toBeNull();
  });

  it("reaches outside the workspace only when told to, as a local image", () => {
    expect(imageRefFor("/Users/me/Exports/promo.png", ROOT, { outside: true })).toEqual({ kind: "local", path: "/Users/me/Exports/promo.png" });
    expect(imageRefFor("~/Exports/promo.png", ROOT, { outside: true })).toEqual({ kind: "local", path: "~/Exports/promo.png" });
    expect(imageRefFor("~/Exports/promo.png", ROOT)).toBeNull();
    expect(imageRefFor("/Users/me/../etc/x.png", ROOT, { outside: true })).toBeNull();
    // A workspace path stays a workspace image either way.
    expect(imageRefFor(`${ROOT}/a.png`, ROOT, { outside: true })).toEqual({ kind: "workspace", relative: "a.png" });
  });

  it("looks for bare file names in the folder a reply names", () => {
    const text = "I checked both versions. They're in ~/Projects/archive/launch/promo/:\n\n- **atlas-launch.png** (wide)\n- `atlas-launch-square.png`";
    expect(foldersIn(text)).toEqual(["~/Projects/archive/launch/promo"]);
    expect(imageRefsIn(text, ROOT, { outside: true })).toEqual([
      { kind: "local", path: "~/Projects/archive/launch/promo/atlas-launch.png" },
      { kind: "workspace", relative: "atlas-launch.png" },
      { kind: "local", path: "~/Projects/archive/launch/promo/atlas-launch-square.png" },
      { kind: "workspace", relative: "atlas-launch-square.png" },
    ]);
    // Without reach, nothing outside is looked for.
    expect(imageRefsIn(text, ROOT)).toEqual([
      { kind: "workspace", relative: "atlas-launch.png" },
      { kind: "workspace", relative: "atlas-launch-square.png" },
    ]);
  });

  it("does not take short words or image paths for folders", () => {
    expect(foldersIn("use and/or /usr or /tmp; see /Users/me/a.png")).toEqual([]);
    expect(foldersIn("saved to /Users/me/Exports and ~/Desktop/")).toEqual(["/Users/me/Exports", "~/Desktop"]);
  });

  it("finds each image in text once, in order, without reading an absolute path twice", () => {
    const text = `Saved \`assets/app-logo-v1.png\` — 1254×1254. Copied from ${GENERATED} to ${ROOT}/assets/app-logo-v1.png.`;
    expect(imageRefsIn(text, ROOT)).toEqual([
      { kind: "workspace", relative: "assets/app-logo-v1.png" },
      { kind: "generated", path: GENERATED },
    ]);
  });
});

const tool = (toolCallId: string, extra: Partial<ToolPatch>): ToolPatch => ({ toolCallId, present: [], cleared: [], ...extra }) as ToolPatch;
const agent = (key: string, text: string): Card => ({ kind: "message", key, message: { key, role: "agent", messageId: key, blocks: [{ type: "text", text }], contentUnknown: false } });

describe("placing images in the transcript", () => {
  it("shows an image once, at the read or generation that brought it in, and never for a shell command", () => {
    const tools = new Map<string, ToolPatch>([
      ["ls", tool("ls", { toolKind: "execute", rawInput: { command: `ls -la ${ROOT}/assets/app-logo-v1.png` } })],
      ["read", tool("read", { toolKind: "read", rawInput: { file_path: `${ROOT}/assets/app-logo-v1.png` } })],
      ["gen", tool("gen", { toolKind: "other", rawOutput: { savedPath: GENERATED }, content: [{ path: GENERATED }] })],
    ]);
    const cards: Card[] = [
      { kind: "tool", key: "c-ls", toolCallId: "ls" },
      { kind: "tool", key: "c-read", toolCallId: "read" },
      agent("m1", "Verified — `assets/app-logo-v1.png` is in place."),
      { kind: "tool", key: "c-gen", toolCallId: "gen" },
    ];
    const placed = assignImages(cards, tools, ROOT);
    expect(placed.has("c-ls")).toBe(false);
    expect(placed.get("c-read")).toEqual([{ kind: "workspace", relative: "assets/app-logo-v1.png" }]);
    expect(placed.has("m1")).toBe(false);
    expect(placed.get("c-gen")).toEqual([{ kind: "generated", path: GENERATED }]);
  });

  it("shows an image the agent only names in its reply", () => {
    const placed = assignImages([agent("m", "Created assets/icon.webp for the store page.")], new Map(), ROOT);
    expect(placed.get("m")).toEqual([{ kind: "workspace", relative: "assets/icon.webp" }]);
  });
});

describe("scanning stays cheap on a long transcript", () => {
  it("reads a bounded amount of a huge tool output, without serialising it", () => {
    const huge = { stdout: "x".repeat(5_000_000), nested: [{ path: "assets/a.png" }] };
    const text = textOf(huge, 1000);
    expect(text.length).toBeLessThanOrEqual(1000);
    expect(textOf({ a: "one", b: ["two", { c: "three" }] })).toBe("one\ntwo\nthree");
  });

  it("scans each card once, and again only when it changes", () => {
    const patch = tool("read", { toolKind: "read", rawInput: { file_path: `${ROOT}/a.png` } });
    const tools = new Map([["read", patch]]);
    const cards: Card[] = [{ kind: "tool", key: "c", toolCallId: "read" }];
    const first = assignImages(cards, tools, ROOT);
    expect(first.get("c")).toEqual([{ kind: "workspace", relative: "a.png" }]);
    // The same object again: the cached answer, even though the path moved.
    (patch.rawInput as { file_path: string }).file_path = `${ROOT}/b.png`;
    expect(assignImages(cards, tools, ROOT).get("c")).toEqual([{ kind: "workspace", relative: "a.png" }]);
    // A new object (what an update produces): scanned again.
    tools.set("read", { ...patch, rawInput: { file_path: `${ROOT}/b.png` } });
    expect(assignImages(cards, tools, ROOT).get("c")).toEqual([{ kind: "workspace", relative: "b.png" }]);
  });
});

describe("an image tool's saved file", () => {
  const SAVED = "/Users/me/Library/Application Support/dev.thingmaker.desktop/accounts/codex/acct-6d/generated_images/01a1/exec-e7.png";
  const patch = (savedPath: string): ToolPatch =>
    ({ toolCallId: "ig", title: "Generate image", status: "completed", toolKind: "other", name: "image_generation", content: null, rawInput: { prompt: "a wizard" }, rawOutput: { savedPath, revisedPrompt: "a wizard" }, locations: [{ path: savedPath }], meta: null, present: [], cleared: [] }) as ToolPatch;
  const card = { kind: "tool", key: "k-ig", toolCallId: "ig" } as unknown as Card;

  it("shows under the tool call, from an extra Codex account's home with a space in its path", () => {
    const images = assignImages([card], new Map([["ig", patch(SAVED)]]), ROOT);
    expect(images.get("k-ig")).toEqual([{ kind: "generated", path: SAVED }]);
  });

  it("is read from the workspace when it was saved there", () => {
    const images = assignImages([card], new Map([["ig", patch(`${ROOT}/art/wizard.png`)]]), ROOT);
    expect(images.get("k-ig")).toEqual([{ kind: "workspace", relative: "art/wizard.png" }]);
  });

  it("knows the default Codex home and extra accounts' homes as generated images", () => {
    expect(imageRefFor(GENERATED, ROOT)).toEqual({ kind: "generated", path: GENERATED });
    expect(imageRefFor("/d/accounts/codex/acct-1/generated_images/t/x.png", ROOT)).toEqual({ kind: "generated", path: "/d/accounts/codex/acct-1/generated_images/t/x.png" });
    expect(imageRefFor("/d/accounts/claude/acct-1/generated_images/t/x.png", ROOT)).toBeNull();
  });
});
