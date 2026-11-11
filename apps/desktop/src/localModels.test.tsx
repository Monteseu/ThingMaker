/**
 * Local Models on screen: the Providers card walks through setup (start
 * Ollama, get a model), the model menu offers a local model as a chat or an
 * agent, and a team keeps a local lead's work on this computer.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseLocalModel, localModelId, type Combo, type LocalStatus, type OrchestratorChoice, type ProviderModel } from "@thingmaker/contracts";

const calls = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));
vi.mock("./ipc", () => ({
  api: new Proxy(calls.api, { get: (target, key: string) => (target[key] ??= vi.fn(async () => undefined)) }),
}));
import { api } from "./ipc";
import { useStore } from "./store";
import { freeNotPrivate, isLocalChat } from "./localModels";
import { suggestModel } from "./teamRoles";
import { LocalModelsCard } from "./components/LocalModelsCard";
import { ConfigPickers } from "./components/ConfigPickers";
import { TeamChart } from "./components/TeamChart";
import { LocalBadge } from "./components/LocalBadge";

const model = (id: string, name: string, description: string): ProviderModel => ({ id, name, description, needsCredits: false, efforts: [], inputImage: false, isDefault: false });
const localCatalog = [model("@chat/llama3.1:8b", "llama3.1:8b", "Chat · 8B"), model("@chat/qwen3:8b", "qwen3:8b", "Chat · 8B"), model("@agent/qwen3:8b", "qwen3:8b", "Agent · 8B")];

const status = (extra: Partial<LocalStatus> = {}): LocalStatus => ({
  installed: true,
  running: true,
  version: "0.40.16",
  models: [
    { name: "llama3.1:8b", family: "llama", parameterSize: "8.0B", quantization: "Q4_K_M", sizeBytes: 4_900_000_000, contextLength: 131072, capabilities: ["completion"], fit: "fits" },
    { name: "qwen3:8b", family: "qwen3", parameterSize: "8.2B", quantization: "Q4_K_M", sizeBytes: 5_200_000_000, contextLength: 40960, capabilities: ["completion", "tools", "thinking"], fit: "fits" },
  ],
  loaded: [],
  recommended: [
    { name: "qwen2.5-coder:7b", purpose: "Code", sizeBytes: 4_700_000_000, agent: true, installed: false, fit: "fits" },
    { name: "gpt-oss:20b", purpose: "Stronger reasoning", sizeBytes: 14_000_000_000, agent: true, installed: false, fit: "too_big" },
  ],
  memoryBytes: 17_179_869_184,
  privateMode: "sandbox",
  agentsAvailable: true,
  ...extra,
});

beforeEach(() => {
  for (const key of Object.keys(calls.api)) delete calls.api[key];
  useStore.setState({ providerModels: { local: localCatalog }, providers: [{ provider: "local", label: "Local Models", resolved: { program: "ollama" }, auth: { loggedIn: true } }], quotas: {}, helperPresets: [], loadHelperPresets: vi.fn(async () => undefined), loadProviderModels: vi.fn(async () => []), loadProviders: vi.fn(async () => undefined) } as never);
});
afterEach(cleanup);

describe("local model ids and sessions", () => {
  it("carry how they run, and a chat is told apart by its agent", () => {
    expect(parseLocalModel("@agent/qwen3:8b")).toEqual({ kind: "agent", name: "qwen3:8b" });
    expect(parseLocalModel("hf.co/org/model:Q4")).toEqual({ kind: "chat", name: "hf.co/org/model:Q4" });
    expect(localModelId("chat", "llama3.1:8b")).toBe("@chat/llama3.1:8b");
    expect(isLocalChat({ provider: "local", capabilities: { agentName: "thingmaker-local-chat" } })).toBe(true);
    expect(isLocalChat({ provider: "local", capabilities: { agentName: "@agentclientprotocol/claude-agent-acp" } })).toBe(false);
    expect(freeNotPrivate("claude", "local")).toBe(true);
    expect(freeNotPrivate("local", "local")).toBe(false);
  });
});

describe("the Local Models card", () => {
  const info = { provider: "local" as const, label: "Local Models", resolved: { provider: "local", program: "/usr/local/bin/ollama", prefixArgs: [], source: "discovered" }, auth: { provider: "local", loggedIn: false } } as never;

  it("starts Ollama when it is stopped, then lists what is installed and what each can be", async () => {
    vi.mocked(api.localStatus).mockResolvedValueOnce(status({ running: false, models: [], recommended: [] })).mockResolvedValue(status());
    vi.mocked(api.localStart).mockResolvedValue("0.40.16");
    render(<LocalModelsCard info={info} />);
    fireEvent.click(await screen.findByRole("button", { name: "Start Ollama" }));
    await waitFor(() => expect(api.localStart).toHaveBeenCalled());
    expect(await screen.findByText("qwen3:8b")).toBeTruthy();
    const llama = screen.getByText("llama3.1:8b").closest("tr")!;
    expect(within(llama).getByText("Chat only")).toBeTruthy();
    expect(within(screen.getByText("qwen3:8b").closest("tr")!).getByText("Chat · Agent")).toBeTruthy();
  });

  it("downloads a model with progress, and will not offer one too big for this computer", async () => {
    vi.mocked(api.localStatus).mockResolvedValue(status());
    let send: ((event: unknown) => void) | undefined;
    vi.mocked(api.localPull).mockImplementation(async (_name: string, onEvent: (event: never) => void) => {
      send = onEvent as never;
    });
    render(<LocalModelsCard info={info} />);
    const coder = (await screen.findByText("qwen2.5-coder:7b")).closest(".local-get") as HTMLElement;
    expect((within(screen.getByText("gpt-oss:20b").closest(".local-get") as HTMLElement).getByRole("button", { name: "Download" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(within(coder).getByRole("button", { name: "Download" }));
    await waitFor(() => expect(api.localPull).toHaveBeenCalledWith("qwen2.5-coder:7b", expect.any(Function)));
    send?.({ kind: "progress", status: "pulling", completed: 50, total: 100 });
    expect(await within(coder).findByRole("button", { name: /50% · Cancel/ })).toBeTruthy();
    fireEvent.click(within(coder).getByRole("button", { name: /Cancel/ }));
    expect(api.localPullCancel).toHaveBeenCalledWith("qwen2.5-coder:7b");
  });

  it("asks Windows, once, to let local agents reach Ollama", async () => {
    vi.mocked(api.localStatus).mockResolvedValueOnce(status({ privateMode: "app_container", loopbackAllowed: false })).mockResolvedValue(status({ privateMode: "app_container", loopbackAllowed: true }));
    vi.mocked(api.localAllowLoopback).mockResolvedValue(true);
    render(<LocalModelsCard info={info} />);
    fireEvent.click(await screen.findByRole("button", { name: "Allow" }));
    await waitFor(() => expect(api.localAllowLoopback).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole("button", { name: "Allow" })).toBeNull());
  });

  it("points to Ollama's download when it is not installed", async () => {
    const openUrl = vi.fn(async () => undefined);
    useStore.setState({ openUrl } as never);
    vi.mocked(api.localStatus).mockResolvedValue(status({ installed: false, running: false, models: [], recommended: [] }));
    render(<LocalModelsCard info={{ provider: "local", label: "Local Models", problem: "Ollama is not installed." } as never} />);
    fireEvent.click(await screen.findByRole("button", { name: "Get Ollama" }));
    expect(openUrl).toHaveBeenCalledWith("https://ollama.com/download");
  });
});

describe("the model menu", () => {
  it("offers a local model as a chat or an agent", () => {
    const options = [{ id: "model", name: "Model", category: "model", type: "select", currentValue: "@chat/llama3.1:8b", options: localCatalog.map((entry) => ({ value: entry.id, name: entry.name, description: entry.description })) }];
    render(<ConfigPickers configOptions={options} disabled={false} sessionId="s1" />);
    const chip = screen.getByRole("button", { name: /llama3.1:8b/ });
    expect(chip.textContent).toContain("Chat");
    fireEvent.click(chip);
    const list = screen.getByRole("listbox", { name: "Model" });
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["llama3.1:8b8B", "qwen3:8b8B"]);
    fireEvent.click(screen.getByRole("button", { name: "Agent" }));
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["qwen3:8b8B"]);
    expect(screen.getByText(/sandbox that reaches only this computer/)).toBeTruthy();
  });
});

describe("a team with local models", () => {
  let orchestrator: OrchestratorChoice;
  let combo: Combo;
  const onChange = vi.fn((next: OrchestratorChoice, nextCombo: Combo) => {
    orchestrator = next;
    combo = nextCombo;
  });

  it("keeps a local lead's helpers local, and offers only models that can be agents", () => {
    orchestrator = { provider: "local", model: "@agent/qwen3:8b" };
    combo = { workers: [{ name: "Cloud", provider: "claude", capabilities: ["code"] }], nativeSubagents: false };
    useStore.setState({ providers: [{ provider: "local", resolved: {}, auth: { loggedIn: true } }, { provider: "claude", resolved: {}, auth: { loggedIn: true } }] } as never);
    render(<TeamChart combo={combo} onChange={onChange} orchestrator={orchestrator} />);
    expect(screen.getByText("A local lead does not hire it: its work would leave this computer")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Add a helper/ }));
    const picker = screen.getByRole("region", { name: "Add a helper" });
    expect(within(picker).getAllByText("not with a local lead")).toHaveLength(3);
    const local = within(picker).getAllByRole("button", { name: /qwen3:8b/ });
    expect(local).toHaveLength(1);
    expect(within(picker).queryByRole("button", { name: /llama3.1:8b/ })).toBeNull();
    fireEvent.click(local[0]!);
    expect(combo.workers[1]).toMatchObject({ provider: "local", model: "@agent/qwen3:8b" });
  });
});

describe("a local chat", () => {
  it("can continue as an agent on the same model, after asking", async () => {
    useStore.setState({
      sessions: { s1: { snapshot: { provider: "local", capabilities: { agentName: "thingmaker-local-chat" } } } },
      pendingProvider: {},
    } as never);
    vi.mocked(api.confirmDialog).mockResolvedValue(true);
    const options = [{ id: "model", name: "Model", category: "model", type: "select", currentValue: "qwen3:8b", options: [{ value: "qwen3:8b", name: "qwen3:8b" }] }];
    render(<LocalBadge configOptions={options} localChat sessionId="s1" />);
    expect(screen.getByText("Private · on this computer · free")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continue as an agent" }));
    await waitFor(() => expect(useStore.getState().pendingProvider.s1).toEqual({ provider: "local", model: "@agent/qwen3:8b" }));
    // Choosing the chat it already is changes nothing.
    useStore.getState().setPendingProvider("s1", { provider: "local", model: "@chat/qwen3:8b" });
    expect(useStore.getState().pendingProvider.s1).toBeUndefined();
  });

  it("offers no agent when its model cannot use tools", () => {
    const options = [{ id: "model", name: "Model", category: "model", type: "select", currentValue: "llama3.1:8b", options: [] }];
    render(<LocalBadge configOptions={options} localChat sessionId="s1" />);
    expect(screen.queryByRole("button", { name: "Continue as an agent" })).toBeNull();
  });
});

describe("suggested helpers", () => {
  it("are local agents under a local lead, and never a model that only chats", () => {
    const catalogs = { local: [...localCatalog, model("@agent/qwen2.5-coder:7b", "qwen2.5-coder:7b", "Agent · 7B")], claude: [model("sonnet", "Sonnet 5.5", "")] };
    expect(suggestModel("coder", catalogs, ["local", "claude"], "local")).toMatchObject({ provider: "local", model: { id: "@agent/qwen2.5-coder:7b" } });
    expect(suggestModel("researcher", catalogs, ["local", "claude"], "local").model?.id).toBe("@agent/qwen3:8b");
    expect(suggestModel("researcher", { local: localCatalog }, ["local"], "claude").model?.id).toBe("@agent/qwen3:8b");
  });
});

describe("starting a local session", () => {
  it("opens on the model the chip shows when none was picked", async () => {
    useStore.setState({
      sessions: { s1: { workspaceId: "w1", handle: { id: "s1" }, snapshot: { provider: "claude" }, projection: { cards: [] }, inFlightRequestId: null } },
      pendingProvider: { s1: { provider: "local" } },
      drafts: { s1: "Hi" },
      attachments: {},
      mentions: {},
      teams: {},
    } as never);
    vi.mocked(api.sessionOpen).mockRejectedValue(new Error("stop here"));
    await useStore.getState().continueOnProvider("s1");
    expect(api.sessionOpen).toHaveBeenCalledWith(expect.objectContaining({ provider: "local", model: "@chat/llama3.1:8b" }));
  });
});
