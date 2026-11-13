import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { MemberResourceCatalog, WorkerSlot } from "@thingmaker/contracts";

vi.mock("./ipc", () => ({ api: { memberResources: vi.fn() } }));
import { api } from "./ipc";
import { MemberResources } from "./components/MemberResources";
import { useStore } from "./store";

const catalog: MemberResourceCatalog = { problem: null, notes: [], entries: [
  { id: "skill:user:art", name: "art", kind: "skill", description: "Draw", group: "user", available: true, problem: null },
  { id: "skill:qa@market:check", name: "qa:check", kind: "skill", description: "Check", group: "plugin:qa@market", available: true, problem: null },
  { id: "mcp:user:paint", name: "paint", kind: "mcp", description: "MCP server", group: "user", available: true, problem: null },
] };
const initial: WorkerSlot = { name: "Artist", provider: "claude", capabilities: ["image"] };
let latest: WorkerSlot;
function Editor({ worker = initial }: { worker?: WorkerSlot }) {
  const [value, setValue] = useState(worker);
  latest = value;
  return <MemberResources worker={value} onChange={setValue} />;
}
beforeEach(() => {
  vi.mocked(api.memberResources).mockReset().mockResolvedValue(catalog);
  useStore.setState({ workspaces: [{ id: "project", displayPath: "My project", canonicalRoot: "/project" }] } as never);
});
afterEach(cleanup);

it("starts custom mode empty and keeps skill and MCP selections independent", async () => {
  render(<Editor />);
  expect(api.memberResources).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Resource mode"), { target: { value: "custom" } });
  await screen.findByRole("checkbox", { name: "art" });
  expect(latest.resources).toEqual({ skills: [], mcps: [] });
  fireEvent.click(screen.getByRole("checkbox", { name: "art" }));
  fireEvent.click(screen.getByRole("button", { name: /MCPs/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: "paint" }));
  expect(latest.resources).toEqual({ skills: ["skill:user:art"], mcps: ["mcp:user:paint"] });
  fireEvent.change(screen.getByLabelText("Resource mode"), { target: { value: "inherit" } });
  expect(latest.resources).toBeUndefined();
});

it("selects a snapshot of a group and reports missing imported references", async () => {
  render(<Editor worker={{ ...initial, resources: { skills: ["skill:project:missing"], mcps: [] } }} />);
  await screen.findByText(/Some selected resources are missing/);
  fireEvent.change(screen.getByRole("textbox", { name: "Search resources" }), { target: { value: "qa:" } });
  fireEvent.click(screen.getByRole("button", { name: "Select group" }));
  expect(latest.resources?.skills).toEqual(["skill:project:missing", "skill:qa@market:check"]);
  expect(latest.resources?.skills).not.toContain("skill:user:art");
  fireEvent.change(screen.getByLabelText("Preview project"), { target: { value: "project" } });
  await waitFor(() => expect(api.memberResources).toHaveBeenLastCalledWith("project", "claude"));
});

it("preserves a policy when switching to another provider until explicitly restored", () => {
  render(<Editor worker={{ ...initial, provider: "gemini", resources: { skills: [], mcps: [] } }} />);
  expect(screen.getByRole("alert").textContent).toContain("requires Claude Code or Codex");
  expect(api.memberResources).not.toHaveBeenCalled();
  expect(latest.resources).toEqual({ skills: [], mcps: [] });
});

it("uses the same selector for Codex and lets the person search by skill source", async () => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [{ ...catalog.entries[0]!, id: "skill:agents-user:art", source: ".agents/skills" }] });
  render(<Editor worker={{ ...initial, provider: "codex", resources: { skills: [], mcps: [] } }} />);
  await screen.findByRole("checkbox", { name: "art" });
  expect(api.memberResources).toHaveBeenCalledWith(null, "codex");
  fireEvent.change(screen.getByRole("textbox", { name: "Search resources" }), { target: { value: ".agents" } });
  fireEvent.click(screen.getByRole("checkbox", { name: "art" }));
  expect(latest.resources?.skills).toEqual(["skill:agents-user:art"]);
});

it.each(["claude", "codex"] as const)("detects selected-skill connections and explicitly activates them for %s", async (provider) => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [
    { ...catalog.entries[0]!, mcpDependencies: [{ name: "paint", evidence: "declared", required: true, mcpIds: ["mcp:user:paint"] }] },
    catalog.entries[2]!,
  ] });
  render(<Editor worker={{ ...initial, provider, resources: { skills: [], mcps: [] } }} />);
  fireEvent.click(await screen.findByRole("checkbox", { name: "art" }));
  const dependencies = screen.getByRole("region", { name: "Connections for selected skills" });
  expect(within(dependencies).getByText("Required connection not selected")).toBeTruthy();
  expect(latest.resources?.mcps, "selecting a skill does not activate any server").toEqual([]);
  fireEvent.click(within(dependencies).getByRole("button", { name: "Activate unambiguous connections" }));
  expect(latest.resources).toEqual({ skills: ["skill:user:art"], mcps: ["mcp:user:paint"] });
  expect(within(dependencies).getByText("Connection active")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /MCPs/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: "paint" }));
  expect(within(dependencies).getByText("Required connection not selected")).toBeTruthy();
  expect(latest.resources?.skills).toEqual(["skill:user:art"]);
});

it("requires a choice between origins, reports missing connections and preserves MCPs when a skill is removed", async () => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [
    { ...catalog.entries[0]!, mcpDependencies: [
      { name: "paint", evidence: "reference", required: false, mcpIds: ["mcp:user:paint", "mcp:project:paint"] },
      { name: "missing", evidence: "declared", required: true, mcpIds: [] },
    ] },
    catalog.entries[2]!, { ...catalog.entries[2]!, id: "mcp:project:paint", group: "project" },
  ] });
  render(<Editor worker={{ ...initial, resources: { skills: [], mcps: [] } }} />);
  fireEvent.click(await screen.findByRole("checkbox", { name: "art" }));
  const dependencies = screen.getByRole("region", { name: "Connections for selected skills" });
  expect(within(dependencies).getByText(/No configured MCP matches/)).toBeTruthy();
  expect(within(dependencies).getByText(/Several connections match/)).toBeTruthy();
  expect(within(dependencies).queryByRole("button", { name: "Activate unambiguous connections" })).toBeNull();
  fireEvent.click(within(dependencies).getByRole("checkbox", { name: /Activate paint · project/ }));
  expect(latest.resources?.mcps).toEqual(["mcp:project:paint"]);
  fireEvent.click(within(dependencies).getByRole("checkbox", { name: /Activate paint · user/ }));
  expect(latest.resources?.mcps, "explicit origin choice replaces the conflicting runtime name").toEqual(["mcp:user:paint"]);
  fireEvent.click(screen.getByRole("checkbox", { name: "art" }));
  expect(latest.resources?.mcps).toEqual(["mcp:user:paint"]);
  expect(screen.queryByRole("region", { name: "Connections for selected skills" })).toBeNull();
});

it("shows disabled dependencies without offering to enable them", async () => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [
    { ...catalog.entries[0]!, mcpDependencies: [{ name: "paint", evidence: "declared", required: true, mcpIds: ["mcp:user:paint"] }] },
    { ...catalog.entries[2]!, available: false, problem: "Unsupported transport" },
  ] });
  render(<Editor worker={{ ...initial, resources: { skills: ["skill:user:art"], mcps: [] } }} />);
  const dependencies = await screen.findByRole("region", { name: "Connections for selected skills" });
  expect((within(dependencies).getByRole("checkbox") as HTMLInputElement).disabled).toBe(true);
  expect(within(dependencies).getByText("Unsupported transport")).toBeTruthy();
  expect(latest.resources?.mcps).toEqual([]);
});

it("never switches on an unapproved project server and shows what each server runs", async () => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [
    { ...catalog.entries[0]!, mcpDependencies: [{ name: "paint", evidence: "declared", required: true, mcpIds: ["mcp:project:paint", "mcp:user:paint"] }] },
    { ...catalog.entries[2]!, target: "paint.example.test" },
    { ...catalog.entries[2]!, id: "mcp:project:paint", group: "project", target: "paint-cli", available: false, problem: "Project server not approved in Claude Code. Approve it there, then refresh." },
  ] });
  render(<Editor worker={{ ...initial, resources: { skills: ["skill:user:art"], mcps: [] } }} />);
  const dependencies = await screen.findByRole("region", { name: "Connections for selected skills" });
  expect(within(dependencies).getByText(/paint-cli/)).toBeTruthy();
  fireEvent.click(within(dependencies).getByRole("button", { name: "Activate unambiguous connections" }));
  expect(latest.resources?.mcps, "only the approved definition is a candidate").toEqual(["mcp:user:paint"]);
  fireEvent.click(screen.getByRole("button", { name: /MCPs/ }));
  expect(screen.getByText("paint.example.test")).toBeTruthy();
  expect((screen.getAllByRole("checkbox", { name: "paint" }).find((box) => !(box as HTMLInputElement).checked) as HTMLInputElement).disabled).toBe(true);
});

it("points a missing connection to Settings › Integrations", async () => {
  vi.mocked(api.memberResources).mockResolvedValue({ ...catalog, entries: [
    { ...catalog.entries[0]!, mcpDependencies: [{ name: "missing", evidence: "declared", required: true, mcpIds: [] }] },
  ] });
  render(<Editor worker={{ ...initial, resources: { skills: ["skill:user:art"], mcps: [] } }} />);
  const dependencies = await screen.findByRole("region", { name: "Connections for selected skills" });
  fireEvent.click(within(dependencies).getByRole("button", { name: "Open integration configuration" }));
  expect(useStore.getState().view).toEqual({ kind: "settings", tab: "integrations" });
});
