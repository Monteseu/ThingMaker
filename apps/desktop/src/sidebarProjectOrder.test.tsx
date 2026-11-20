import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { WorkspaceRecord } from "@thingmaker/contracts";
import { afterEach, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { Sidebar } from "./components/Sidebar";
import { useStore } from "./store";

const initial = useStore.getState();
const orderKey = "thingmaker.sidebar.projectOrder";
afterEach(() => {
  cleanup();
  localStorage.removeItem(orderKey);
  useStore.setState(initial, true);
});

function workspace(id: string, createdAt: number): WorkspaceRecord {
  const path = `/projects/${id}`;
  return { id, environmentId: "e", canonicalRoot: path, displayPath: path, workspaceHash: id, trustState: "trusted_local", createdAt };
}

const project = (name: string) => screen.getByRole("button", { name });
const names = () => Array.from(document.querySelectorAll(".ws-name .sidebar-title"), (title) => title.textContent);
const moveItem = (name: string) => screen.getByRole<HTMLButtonElement>("menuitem", { name });

it("moves projects in either direction, respects the list ends, and restores the order on remount", () => {
  const alpha = workspace("Alpha", 3);
  const beta = workspace("Beta", 2);
  const gamma = workspace("Gamma", 1);
  const quick = workspace("Quick", 4);
  useStore.setState({ workspaces: [quick, alpha, beta, gamma], quickChats: [{ workspaceId: quick.id, createdAt: 4, temporary: false }] });
  const sidebar = render(<Sidebar />);
  expect(names()).toEqual(["Alpha", "Beta", "Gamma"]);

  fireEvent.contextMenu(project("Alpha"));
  expect(moveItem("Move up").disabled).toBe(true);
  fireEvent.click(moveItem("Move down"));
  expect(names()).toEqual(["Beta", "Alpha", "Gamma"]);

  const trigger = project("Gamma");
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "F10", shiftKey: true });
  expect(moveItem("Move down").disabled).toBe(true);
  fireEvent.click(moveItem("Move up"));
  expect(names()).toEqual(["Beta", "Gamma", "Alpha"]);
  expect(document.activeElement).toBe(trigger);

  sidebar.unmount();
  render(<Sidebar />);
  expect(names()).toEqual(["Beta", "Gamma", "Alpha"]);

  act(() => useStore.setState({ workspaces: [workspace("Epsilon", 6), workspace("Delta", 5), quick, alpha, gamma] }));
  expect(names()).toEqual(["Gamma", "Alpha", "Delta", "Epsilon"]);
});

it("keeps the default order if the saved preference is invalid", () => {
  useStore.setState({ workspaces: [workspace("Beta", 2), workspace("Alpha", 1)] });
  localStorage.setItem(orderKey, "null");
  render(<Sidebar />);
  expect(names()).toEqual(["Beta", "Alpha"]);
});
