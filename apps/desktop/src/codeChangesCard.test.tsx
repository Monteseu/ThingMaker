/** The Code changes card: one line until it is opened. */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { CodeChangesCard } from "./components/ChangeCards";

afterEach(cleanup);

it("starts folded and opens from its line, not from a link", async () => {
  const file = (path: string) => ({ path, kind: "modified", additions: 2, deletions: 0, binary: false });
  calls.api.reviewDiff = vi.fn(async () => ({ report: { files: [file("a/one.ts"), file("a/two.ts")], truncated: false } }));
  const { container } = render(<CodeChangesCard refreshKey={1} sessionId="s1" workspaceId="w1" />);
  await waitFor(() => expect(screen.getByText("Code changes")).toBeTruthy());
  expect(container.querySelector(".work-files")).toBeNull();
  expect(container.querySelector(".link")).toBeNull();
  fireEvent.click(screen.getByText("Code changes"));
  expect(container.querySelectorAll(".work-files li")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Hide the changed files" }));
  expect(container.querySelector(".work-files")).toBeNull();
});
