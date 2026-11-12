import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useState } from "react";
import type { Card } from "./projection";
vi.mock("./ipc", () => ({ api: { replySuggest: vi.fn() } }));
import { api } from "./ipc";
import { suggestionMessages, suggestionKey, useReplySuggestion } from "./replySuggestion";

const message = (role: "user" | "agent" | "thought", text: string): Card => ({ kind: "message", key: text, message: { key: text, messageId: text, role, blocks: [{ type: "text", text }], contentUnknown: false } });
const context: [string, string, string] = ["Previous answer", "My message", "Last answer"];
afterEach(() => { cleanup(); vi.useRealTimers(); vi.resetAllMocks(); });

it("sends only the last reply, last user message and reply before it, in chronological order", () => {
  const cards: Card[] = [message("user", "older secret"), message("agent", "old answer"), message("agent", context[0]), message("user", context[1]), message("agent", "working commentary"), { kind: "tool", key: "tool", toolCallId: "private tool output" }, message("thought", "private reasoning"), message("agent", context[2])];
  expect(suggestionMessages(cards)).toEqual(context);
  expect(suggestionMessages([...cards, message("user", "new prompt")])).toBeNull();
  expect(suggestionMessages([message("user", "first prompt"), message("agent", "first answer")])).toBeNull();
  expect(suggestionMessages([message("agent", "x".repeat(65537)), message("user", "prompt"), message("agent", "answer")])).toBeNull();
});

it("accepts with Tab without sending, and dismisses with Escape", () => {
  const send = vi.fn();
  function Composer() {
    const [draft, setDraft] = useState("");
    const [suggestion, setSuggestion] = useState("Show me the tests");
    return <textarea aria-label="Prompt" placeholder={suggestion} value={draft} onChange={(e) => setDraft(e.target.value)} onKeyDown={(e) => {
      if (suggestionKey(e, suggestion, setDraft, () => setSuggestion(""))) return;
      if (e.key === "Enter" && draft) send(draft);
    }} />;
  }
  render(<Composer />);
  const input = screen.getByRole("textbox") as HTMLTextAreaElement;
  fireEvent.keyDown(input, { key: "Tab", shiftKey: true });
  expect(input.value).toBe("");
  fireEvent.keyDown(input, { key: "Enter" });
  expect(send).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: "Tab" });
  expect(input.value).toBe("Show me the tests");
  expect(send).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: "Enter" });
  expect(send).toHaveBeenCalledWith("Show me the tests");
  cleanup();
  render(<Composer />);
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" });
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).placeholder).toBe("");
});

it("ignores a result arriving after typing, even if the draft is cleared", async () => {
  vi.useFakeTimers();
  let resolve!: (text: string) => void;
  vi.mocked(api.replySuggest).mockReturnValue(new Promise((done) => { resolve = done; }));
  const { result, rerender } = renderHook(({ draft }) => useReplySuggestion("a", "sonnet", context, true, draft), { initialProps: { draft: "" } });
  await act(() => vi.advanceTimersByTimeAsync(300));
  expect(api.replySuggest).toHaveBeenCalledWith(context);
  rerender({ draft: "mine" });
  rerender({ draft: "" });
  await act(async () => resolve("late suggestion"));
  expect(result.current.suggestion).toBe("");
  expect(api.replySuggest).toHaveBeenCalledTimes(1);
});

it("drops results on a session change and on a new running turn", async () => {
  vi.useFakeTimers();
  const resolves: ((text: string) => void)[] = [];
  vi.mocked(api.replySuggest).mockImplementation(() => new Promise((done) => resolves.push(done)));
  const { result, rerender } = renderHook(({ session, eligible }) => useReplySuggestion(session, "sonnet", context, eligible, ""), { initialProps: { session: "a", eligible: true } });
  await act(() => vi.advanceTimersByTimeAsync(300));
  rerender({ session: "b", eligible: true });
  await act(async () => resolves[0]!("wrong session"));
  expect(result.current.suggestion).toBe("");
  await act(() => vi.advanceTimersByTimeAsync(300));
  rerender({ session: "b", eligible: false });
  await act(async () => resolves[1]!("old turn"));
  expect(result.current.suggestion).toBe("");
});

it("shows the result, dismisses it and makes no calls when disabled or missing context", async () => {
  vi.useFakeTimers();
  vi.mocked(api.replySuggest).mockResolvedValue("Continue with tests");
  const { result, rerender } = renderHook(({ model }) => useReplySuggestion("a", model, context, true, ""), { initialProps: { model: "" } });
  await act(() => vi.advanceTimersByTimeAsync(300));
  expect(api.replySuggest).not.toHaveBeenCalled();
  rerender({ model: "sonnet" });
  await act(() => vi.advanceTimersByTimeAsync(300));
  expect(result.current.suggestion).toBe("Continue with tests");
  act(() => result.current.dismiss());
  expect(result.current.suggestion).toBe("");
  renderHook(() => useReplySuggestion("b", "sonnet", null, true, ""));
  await act(() => vi.advanceTimersByTimeAsync(300));
  expect(api.replySuggest).toHaveBeenCalledTimes(1);
});

it("invalidates an in-flight suggestion when only the provider changes", async () => {
  vi.useFakeTimers();
  const resolves: ((text: string) => void)[] = [];
  vi.mocked(api.replySuggest).mockImplementation(() => new Promise((done) => resolves.push(done)));
  const { result, rerender } = renderHook(({ provider }: { provider: "claude" | "gemini" }) => useReplySuggestion("a", "shared-model", context, true, "", provider), { initialProps: { provider: "claude" } });
  await act(() => vi.advanceTimersByTimeAsync(300));
  rerender({ provider: "gemini" });
  await act(async () => resolves[0]!("wrong provider"));
  expect(result.current.suggestion).toBe("");
  await act(() => vi.advanceTimersByTimeAsync(300));
  await act(async () => resolves[1]!("correct provider"));
  expect(result.current.suggestion).toBe("correct provider");
});
