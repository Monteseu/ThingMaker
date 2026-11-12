/**
 * The macOS and Linux builds target Safari 13's engine. A regex lookbehind
 * (`(?<=`, `(?<!`) needs Safari 16.4; on an older WebView a pattern built when
 * a module loads throws, and the app's window stays blank. Vitest runs on
 * Node, which accepts it, so this is checked in the source instead.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = __dirname;

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("code the WebView runs", () => {
  it("uses no regex lookbehind", () => {
    const offenders = sources(ROOT).filter((path) => /\(\?<[=!]/.test(readFileSync(path, "utf8")));
    expect(offenders.map((path) => relative(ROOT, path))).toEqual([]);
  });
});
