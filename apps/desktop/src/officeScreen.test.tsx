import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ScreenDevice, ScreenStreamEvent } from "@thingmaker/contracts";

vi.mock("./ipc", () => ({ api: { screenDevices: vi.fn(), screenCapture: vi.fn(), screenStreamStart: vi.fn(), screenStreamStop: vi.fn(), screenStreamAck: vi.fn(), screenStreamHeartbeat: vi.fn(), screenToolsStatus: vi.fn(), screenToolsInstall: vi.fn() } }));
import { api } from "./ipc";
import { OfficeScreen } from "./components/OfficeScreen";

const android: ScreenDevice = { id: "emulator-5554", name: "Pixel", platform: "android" };
const ios: ScreenDevice = { id: "605884A0-E18B-4203-8875-995DCCD31CA2", name: "iPhone", platform: "ios" };
const list = vi.mocked(api.screenDevices);
const capture = vi.mocked(api.screenCapture);
const start = vi.mocked(api.screenStreamStart);
const stop = vi.mocked(api.screenStreamStop);
const ack = vi.mocked(api.screenStreamAck);
const heartbeat = vi.mocked(api.screenStreamHeartbeat);
const toolsStatus = vi.mocked(api.screenToolsStatus);
const toolsInstall = vi.mocked(api.screenToolsInstall);
let onVideo: (event: ScreenStreamEvent) => void;
const frame = "data:image/jpeg;base64,aGVsbG8=";
async function mount() {
  const result = render(<OfficeScreen><div>Office scene</div></OfficeScreen>);
  await act(async () => {});
  return result;
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  list.mockResolvedValue([android, ios]);
  capture.mockResolvedValue(frame);
  start.mockImplementation(async (_device, onEvent) => { onVideo = onEvent; return "stream-1"; });
  stop.mockResolvedValue(); ack.mockResolvedValue(); heartbeat.mockResolvedValue();
  toolsStatus.mockResolvedValue({ missing: [], canInstall: false, manual: null });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("office device screens", () => {
  it("captures only the selected device while its screen is visible", async () => {
    await mount();
    expect(screen.getByText("Office scene")).toBeTruthy();
    expect(capture).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    expect(screen.queryByText("Office scene")).toBeNull();
    expect(start).toHaveBeenLastCalledWith(android, expect.any(Function));
    expect(capture).not.toHaveBeenCalled();
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    const image = screen.getByAltText("Screen of Pixel");
    expect(image.getAttribute("src")).toBe(frame);
    fireEvent.load(image);
    expect(ack).toHaveBeenCalledWith("stream-1", 1);
    act(() => onVideo({ kind: "frame", sequence: 2, dataUrl: frame }));
    expect(ack).toHaveBeenCalledWith("stream-1", 2);
    fireEvent.change(screen.getByLabelText("Phone or emulator"), { target: { value: `ios:${ios.id}` } });
    await act(async () => {});
    expect(stop).toHaveBeenCalledWith("stream-1");
    expect(capture).toHaveBeenLastCalledWith(ios);
    expect(screen.getByAltText("Screen of iPhone")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Crew" }));
    const count = capture.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    expect(capture).toHaveBeenCalledTimes(count);
    expect(screen.getByText("Office scene")).toBeTruthy();
  });

  it("closes a stream even when startup resolves after unmount", async () => {
    let finish!: (value: string) => void;
    start.mockImplementationOnce((_device, onEvent) => {
      onVideo = onEvent;
      return new Promise((resolve) => { finish = resolve; });
    });
    const mounted = await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    mounted.unmount();
    await act(async () => { finish("late-stream"); });
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    expect(stop).toHaveBeenCalledWith("late-stream");
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(heartbeat).not.toHaveBeenCalled();
  });

  it("removes stale frames on stream failure and allows retry", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    expect(screen.getByAltText("Screen of Pixel")).toBeTruthy();
    act(() => onVideo({ kind: "error", message: "disconnected" }));
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("scrcpy and FFmpeg");
    expect(screen.getByText("disconnected")).toBeTruthy();
    expect(stop).toHaveBeenCalledWith("stream-1");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await act(async () => {});
    expect(start).toHaveBeenCalledTimes(2);
  });

  it("follows hot-plugged devices and clears the preview when all disconnect", async () => {
    list.mockResolvedValueOnce([]);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen" }));
    expect(screen.getByRole("status").textContent).toContain("No devices found");
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(start).toHaveBeenCalledTimes(1);
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    expect(screen.getByAltText("Screen of Pixel")).toBeTruthy();
    list.mockResolvedValue([]);
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("No devices found");
  });

  it("skips captures while the app is hidden", async () => {
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(list).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    hidden.mockReturnValue(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(start).toHaveBeenCalledTimes(1);
    hidden.mockReturnValue(true);
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(stop).toHaveBeenCalledWith("stream-1");
    hidden.mockReturnValue(false);
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    await act(async () => {});
    expect(start).toHaveBeenCalledTimes(2);
    hidden.mockRestore();
  });

  it("explains when device tools or the desktop bridge are unavailable", async () => {
    list.mockRejectedValue(new Error("unavailable"));
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen" }));
    expect(screen.getByRole("status").textContent).toContain("Android platform tools or Xcode");
    expect(capture).not.toHaveBeenCalled();
  });

  it("stops video when its panel goes out of view and resumes when visible", async () => {
    let intersect!: (entries: { isIntersecting: boolean }[]) => void;
    vi.stubGlobal("IntersectionObserver", class {
      constructor(callback: typeof intersect) { intersect = callback; }
      observe() {}
      disconnect() {}
    });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => intersect([{ isIntersecting: false }]));
    expect(stop).toHaveBeenCalledWith("stream-1");
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(heartbeat).not.toHaveBeenCalled();
    act(() => intersect([{ isIntersecting: true }]));
    await act(async () => {});
    expect(start).toHaveBeenCalledTimes(2);
  });

  it("acknowledges a frame loaded before the stream handle arrives", async () => {
    let finish!: (value: string) => void;
    start.mockImplementationOnce((_device, onEvent) => {
      onVideo = onEvent;
      return new Promise((resolve) => { finish = resolve; });
    });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    act(() => onVideo({ kind: "frame", sequence: 8, dataUrl: frame }));
    fireEvent.load(screen.getByAltText("Screen of Pixel"));
    await act(async () => { finish("late-stream"); });
    expect(ack).toHaveBeenCalledWith("late-stream", 8);
  });

  it("stops the stream and heartbeat if an image cannot be displayed", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    fireEvent.error(screen.getByAltText("Screen of Pixel"));
    expect(stop).toHaveBeenCalledWith("stream-1");
    expect(screen.queryByRole("img")).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(heartbeat).not.toHaveBeenCalled();
  });
  it("offers to install missing video tools and retries once they are in", async () => {
    toolsStatus.mockResolvedValue({ missing: ["scrcpy", "FFmpeg"], canInstall: true, manual: null });
    toolsInstall.mockResolvedValue({ missing: [], canInstall: false, manual: null });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => onVideo({ kind: "error", message: "missing" }));
    await act(async () => {});
    expect(screen.getByText("Missing: scrcpy, FFmpeg")).toBeTruthy();
    const starts = start.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "Install dependencies" }));
    await act(async () => {});
    expect(toolsInstall).toHaveBeenCalledTimes(1);
    expect(start.mock.calls.length).toBe(starts + 1);
  });

  it("reconnects on its own after a failure, backing off, and recovers", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => onVideo({ kind: "error", message: "Timed out starting device video" }));
    await act(async () => {});
    expect(screen.getByText("Timed out starting device video")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(2900); });
    expect(start).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(200); });
    expect(start).toHaveBeenCalledTimes(2);
    act(() => onVideo({ kind: "error", message: "again" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(start).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
    expect(start).toHaveBeenCalledTimes(3);
    act(() => onVideo({ kind: "frame", sequence: 1, dataUrl: frame }));
    expect(screen.getByAltText("Screen of Pixel")).toBeTruthy();
    expect(screen.queryByText("again")).toBeNull();
  });

  it("shows why startup was rejected", async () => {
    start.mockRejectedValueOnce({ message: "Too many device previews are open" });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    expect(screen.getByText("Too many device previews are open")).toBeTruthy();
  });

  it("waits for missing tools instead of retrying, then retries once they appear", async () => {
    toolsStatus.mockResolvedValue({ missing: ["scrcpy"], canInstall: true, manual: null });
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Screen (2)" }));
    await act(async () => {});
    act(() => onVideo({ kind: "error", message: "missing" }));
    await act(async () => {});
    await act(async () => { await vi.advanceTimersByTimeAsync(9000); });
    expect(start).toHaveBeenCalledTimes(1);
    toolsStatus.mockResolvedValue({ missing: [], canInstall: false, manual: null });
    await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
    await act(async () => { await vi.advanceTimersByTimeAsync(3100); });
    expect(start).toHaveBeenCalledTimes(2);
  });

  it("backs off from 3 to 30 seconds", async () => {
    const { retryDelay } = await import("./components/OfficeScreen");
    expect([1, 2, 3, 4, 5, 9].map(retryDelay)).toEqual([3000, 6000, 12000, 24000, 30000, 30000]);
  });
});
