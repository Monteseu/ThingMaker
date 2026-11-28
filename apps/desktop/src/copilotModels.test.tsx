import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { api } from "./ipc";
import { useStore } from "./store";
import { useLanguageForTests } from "./i18n";
import { SignInPanel } from "./components/SignInPanel";

afterEach(() => { cleanup(); useLanguageForTests("en"); vi.resetAllMocks(); });

it.each([
  [true, "Ready"],
  [false, "Not signed in"],
  [undefined, "Not checked yet"],
] as const)("shows Copilot auth status %s without requiring an account name", (loggedIn, expected) => {
  useLanguageForTests("en");
  useStore.setState({
    error: null, accounts: {}, usage: {}, quotas: {}, signIn: null,
    providers: [{ provider: "copilot", label: "GitHub Copilot", resolved: { provider: "copilot", program: "copilot", prefixArgs: [], source: "discovered" }, auth: { provider: "copilot", loggedIn } }],
    providerVersions: {}, providerUpdate: { running: null, results: {} },
    loadAccounts: vi.fn(async () => undefined), loadProviderVersions: vi.fn(async () => undefined),
  } as never);
  render(<SignInPanel />);
  expect(screen.getByText(expected)).toBeTruthy();
});

it.each(["en", "es"] as const)("opens Copilot's empty catalog without a banner or sign-in warning (%s)", async (language) => {
  useLanguageForTests(language);
  vi.mocked(api.providerModels).mockResolvedValue([]);
  useStore.setState({
    error: null, accounts: {}, usage: {}, quotas: {}, signIn: null,
    providers: [{ provider: "copilot", label: "GitHub Copilot", resolved: { provider: "copilot", program: "copilot", prefixArgs: [], source: "discovered" } }],
    providerVersions: {}, providerUpdate: { running: null, results: {} },
    loadAccounts: vi.fn(async () => undefined), loadProviderVersions: vi.fn(async () => undefined),
  } as never);
  render(<SignInPanel />);
  fireEvent.click(screen.getByRole("button", { name: language === "en" ? "More for GitHub Copilot" : "Más de GitHub Copilot" }));
  fireEvent.click(screen.getByRole("menuitem", { name: language === "en" ? "Show models" : "Ver modelos" }));
  expect(await screen.findByText(language === "en"
    ? "Copilot models are chosen after opening a session."
    : "Los modelos de Copilot se eligen al abrir una sesión.")).toBeTruthy();
  expect(api.providerModels).toHaveBeenCalledWith("copilot");
  expect(screen.queryByText(/probably not signed in|probablemente/)).toBeNull();
  expect(useStore.getState().error).toBeNull();
});
