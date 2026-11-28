import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { useStore } from "./store";
import { emptyProjection } from "./projection";
import { QuotaRecovery } from "./components/QuotaRecovery";

const prepare = vi.fn(async () => "next");
const loadAccounts = vi.fn(async () => undefined);

beforeEach(() => {
  prepare.mockReset().mockResolvedValue("next");
  loadAccounts.mockClear();
  const projection = emptyProjection();
  projection.foreground = "failed";
  projection.quotaFailure = true;
  useStore.setState({
    sessions: { s1: { handle: { id: "s1", attachmentGeneration: "1" }, snapshot: { provider: "codex" }, projection, inFlightRequestId: null } },
    providers: ["claude", "codex", "gemini"].map((provider) => ({ provider, resolved: { program: provider }, auth: { loggedIn: provider !== "claude" } })),
    accounts: {}, usage: {}, drafts: { s1: "My unsent draft" }, error: null,
    loadAccounts, prepareQuotaContinuation: prepare,
  } as never);
});
afterEach(cleanup);

function chooseGemini() {
  fireEvent.click(screen.getByRole("button", { name: "Continue with another account or provider" }));
  fireEvent.change(screen.getByRole("combobox", { name: "Continue with" }), { target: { value: "gemini" } });
}

describe("quota continuation choices", () => {
  it("prepares only after an explicit choice and leaves the draft editable", async () => {
    render(<QuotaRecovery sessionId="s1" />);
    expect(prepare).not.toHaveBeenCalled();
    chooseGemini();
    expect(prepare).not.toHaveBeenCalled();
    expect(loadAccounts).toHaveBeenCalledTimes(4);
    expect(screen.getByText(/nothing is sent until you send it/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Prepare continuation" }));
    await waitFor(() => expect(prepare).toHaveBeenCalledWith("s1", { provider: "gemini" }));
    expect(useStore.getState().drafts.s1).toBe("My unsent draft");
  });

  it("does not treat another session's provider-wide usage as this session's exhaustion", () => {
    const session = useStore.getState().sessions.s1!;
    useStore.setState({ sessions: { s1: { ...session, projection: { ...session.projection, quotaFailure: false } } }, usage: { codex: { limitReached: true } } } as never);
    render(<QuotaRecovery sessionId="s1" />);
    expect(screen.queryByRole("region", { name: "Continue after a usage limit" })).toBeNull();
    expect(prepare).not.toHaveBeenCalled();
  });

  it("offers another signed-in account and keeps unavailable providers disabled", async () => {
    useStore.setState({ accounts: { codex: [
      { id: "default", provider: "codex", isDefault: true, active: true, label: "Main", auth: { loggedIn: true } },
      { id: "work", provider: "codex", isDefault: false, active: false, label: "Work", auth: { loggedIn: true } },
    ] } } as never);
    render(<QuotaRecovery sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: "Continue with another account or provider" }));
    expect((screen.getByRole("option", { name: /Claude Code/ }) as HTMLOptionElement).disabled).toBe(true);
    expect(screen.queryByRole("option", { name: /Main account/ })).toBeNull();
    fireEvent.change(screen.getByRole("combobox"), { target: { value: JSON.stringify(["codex", "work"]) } });
    fireEvent.click(screen.getByRole("button", { name: "Prepare continuation" }));
    await waitFor(() => expect(prepare).toHaveBeenCalledWith("s1", { provider: "codex", accountId: "work" }));
  });

  it("prevents duplicate preparations while the first is pending", async () => {
    let finish!: (key: string) => void;
    prepare.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    render(<QuotaRecovery sessionId="s1" />);
    chooseGemini();
    const button = screen.getByRole("button", { name: "Prepare continuation" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(prepare).toHaveBeenCalledTimes(1);
    finish("next");
    await waitFor(() => expect(screen.getByRole("button", { name: "Prepare continuation" })).toHaveProperty("disabled", false));
  });

  it("keeps recovery errors beside the choices and retains the original draft", async () => {
    prepare.mockImplementationOnce(async () => { throw new Error("failed"); });
    render(<QuotaRecovery sessionId="s1" />);
    chooseGemini();
    fireEvent.click(screen.getByRole("button", { name: "Prepare continuation" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "The continuation could not be prepared. Your original conversation is still available.");
    expect(useStore.getState().drafts.s1).toBe("My unsent draft");
  });
});
