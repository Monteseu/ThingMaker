import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ScreenDevice, ScreenTool, ScreenTools, ScreenToolsInstall, ScreenToolsManual, ScreenToolsProblem } from "@thingmaker/contracts";
import { api } from "../ipc";
import { t } from "../i18n";

function deviceKey(device: ScreenDevice): string {
  return `${device.platform}:${device.id}`;
}

/** Sequential captures: never queue frames if a device or its tools are slow. */
function DeviceSnapshots({ device }: { device: ScreenDevice }) {
  const [frame, setFrame] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const capture = async () => {
      if (stopped) return;
      if (!document.hidden) {
        try {
          const next = await api.screenCapture(device);
          if (stopped) return;
          setFrame(next);
          setFailed(false);
        } catch {
          if (stopped) return;
          // Remove the stale frame, so a disconnected device never looks live.
          setFrame(null);
          setFailed(true);
        }
      }
      if (!stopped) timer = setTimeout(() => void capture(), 1000);
    };
    void capture();
    return () => { stopped = true; clearTimeout(timer); };
  }, [device.id, device.platform]);

  return (
    <div className="office-device-screen">
      {frame && <img alt={t("Screen of {name}", { name: device.name })} src={frame} />}
      {!frame && <p role="status">{failed ? t("Cannot read this screen. Check that the device is connected and authorized.") : t("Connecting to the screen…")}</p>}
      <span className="small muted">{t("View only · refreshes every second")}</span>
    </div>
  );
}

function toolName(tool: ScreenTool): string {
  return tool === "adb" ? t("Android platform tools (adb)") : tool === "scrcpy" ? "scrcpy" : "FFmpeg";
}

function manualText(manual: ScreenToolsManual): string {
  switch (manual.kind) {
    case "homebrew": return t("Install Homebrew from {url}, then try again.", { url: manual.url });
    case "winget": return t("Install App Installer from the Microsoft Store ({url}) to get winget, then try again.", { url: manual.url });
    case "command": return t("Install them yourself: {command}", { command: manual.command });
  }
}

function problemText(problem: ScreenToolsProblem | null, tools: ScreenTools): string {
  switch (problem?.kind) {
    case "planChanged": return t("What needs installing has changed. Check the commands and confirm again.");
    case "busy": return t("The device tools are already installing.");
    case "unavailable": return t("The package manager is no longer available.");
    case "cancelled": return t("Install cancelled.");
    case "timedOut": return t("{command} timed out.", { command: problem.command });
    case "couldNotStart": return t("{command} could not start.", { command: problem.command });
    case "failed": return t("{command} failed: {output}", { command: problem.command, output: problem.output });
    default: return t("Installed, but {tools} cannot be found yet. Restart ThingMaker and try again.", { tools: tools.missing.map(toolName).join(", ") });
  }
}

/** The install in progress, shared by every view: one that remounts (Retry,
 * another device, back from the crew) follows it instead of offering a
 * second install that the host would refuse. */
let running: Promise<ScreenToolsInstall> | null = null;

/** Lists the missing tools for live video and, once the person confirms
 * the exact commands, installs them. */
function ScreenToolsSetup({ onInstalled, onMissing }: { onInstalled: () => void; onMissing?: (missing: boolean) => void }) {
  const [tools, setTools] = useState<ScreenTools | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [installing, setInstalling] = useState(running !== null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);
  const follow = async (install: Promise<ScreenToolsInstall>) => {
    setInstalling(true);
    setError(null);
    try {
      const outcome = await install;
      if (!mounted.current) return;
      setTools(outcome.tools);
      if (outcome.tools.missing.length === 0) {
        onMissing?.(false);
        onInstalled();
      } else {
        setError(problemText(outcome.problem, outcome.tools));
      }
    } catch (reason) {
      if (mounted.current) setError(String((reason as { message?: string })?.message ?? reason));
    } finally {
      if (mounted.current) setInstalling(false);
    }
  };
  useEffect(() => {
    mounted.current = true;
    if (running) void follow(running);
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    // Keep checking while something is missing: it may get installed from
    // another window or a terminal, and the caller should then retry.
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const check = () => {
      api.screenToolsStatus().then((next) => {
        if (!live) return;
        setTools(next);
        onMissing?.(next.missing.length > 0);
        if (next.missing.length > 0) timer = setTimeout(check, 10000);
      }).catch(() => {});
    };
    check();
    return () => { live = false; clearTimeout(timer); };
  }, []);
  if (!tools || tools.missing.length === 0) return null;
  const install = () => {
    setConfirming(false);
    running ??= api.screenToolsInstall(tools.plan).finally(() => { running = null; });
    void follow(running);
  };
  return <div className="office-screen-tools">
    <p className="small">{t("Missing: {tools}", { tools: tools.missing.map(toolName).join(", ") })}</p>
    {installing ? <>
      <p className="small" role="status">{t("Installing… this can take a few minutes")}</p>
      <button className="button small" onClick={() => void api.screenToolsCancel().catch(() => {})} type="button">{t("Cancel")}</button>
    </> : confirming ? <>
      <p className="small">{t("ThingMaker will run:")}</p>
      <ul className="office-screen-tools-plan">{tools.plan.map((command) => <li key={command}><code>{command}</code></li>)}</ul>
      {tools.manager === "winget" && <p className="small muted">{t("winget accepts the packages' licence agreements on your behalf.")}</p>}
      <div className="office-screen-tools-actions">
        <button className="button small" onClick={install} type="button">{t("Install")}</button>
        <button className="button small" onClick={() => setConfirming(false)} type="button">{t("Cancel")}</button>
      </div>
    </> : tools.canInstall
      ? <button className="button small" onClick={() => { setError(null); setConfirming(true); }} type="button">{t("Install dependencies")}</button>
      : tools.manual && <p className="small muted">{manualText(tools.manual)}</p>}
    {error && <p className="small error">{error}</p>}
  </div>;
}

/** Delay before reconnecting on its own after a failure: 3 s, doubling to 30 s. */
export function retryDelay(failures: number): number {
  return Math.min(3000 * 2 ** Math.max(0, failures - 1), 30000);
}

/** Continuous scrcpy video: the native side sends a new frame only after
 * the preceding image has loaded, keeping slow views from building a delay.
 * A failed stream reconnects on its own (a phone being plugged in or a tool
 * that just finished installing), except while tools are known missing. */
function LiveVideo({ device, visible }: { device: ScreenDevice; visible: boolean }) {
  const [frame, setFrame] = useState<{ dataUrl: string; sequence: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [reason, setReason] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [check, setCheck] = useState(0);
  const [toolsMissing, setToolsMissing] = useState(false);
  const failures = useRef(0);
  const stream = useRef<string | null>(null);
  const loaded = useRef<number | null>(null);
  const displayed = useRef<string | null>(null);
  const terminate = useRef<() => void>(() => {});
  const acknowledge = (sequence: number) => {
    loaded.current = sequence;
    if (stream.current) {
      void api.screenStreamAck(stream.current, sequence).catch(() => {});
      loaded.current = null;
    }
  };
  useEffect(() => {
    // React keeps an unchanged src; there is no new load event for identical
    // frames. Acknowledge those too, or a static screen would stall the stream.
    if (frame && displayed.current === frame.dataUrl) acknowledge(frame.sequence);
  }, [frame]);
  useEffect(() => {
    setFrame(null);
    setFailed(false);
    loaded.current = null;
    displayed.current = null;
    if (!visible) return;
    let stopped = false;
    let heartbeat: ReturnType<typeof setInterval> | undefined;
    let id: string | null = null;
    const stop = () => {
      stopped = true;
      if (heartbeat) clearInterval(heartbeat);
      if (id) void api.screenStreamStop(id).catch(() => {});
      if (stream.current === id) stream.current = null;
    };
    terminate.current = stop;
    void api.screenStreamStart(device, (event) => {
      if (stopped) return;
      if (event.kind === "frame") {
        failures.current = 0;
        setFrame(event);
        setFailed(false);
        setReason(null);
      } else {
        stopped = true;
        failures.current += 1;
        setFrame(null);
        setFailed(true);
        setReason(event.message || null);
        stop();
      }
    }).then((opened) => {
      id = opened;
      // A view can disappear, change device or fail before startup finishes.
      if (stopped) { stop(); return; }
      stream.current = id;
      if (loaded.current !== null) {
        void api.screenStreamAck(id, loaded.current).catch(() => {});
        loaded.current = null;
      }
      heartbeat = setInterval(() => { void api.screenStreamHeartbeat(opened).catch(() => {}); }, 5000);
    }).catch((error: unknown) => {
      if (stopped) return;
      failures.current += 1;
      setFrame(null);
      setFailed(true);
      setReason((error as { message?: string })?.message || null);
    });
    return () => { stopped = true; stop(); };
  }, [device.id, device.platform, visible, attempt]);
  useEffect(() => {
    // Retrying cannot help while tools are missing; installing them retries.
    if (!failed || !visible || toolsMissing) return;
    const timer = setTimeout(() => setAttempt((value) => value + 1), retryDelay(failures.current));
    return () => clearTimeout(timer);
  }, [failed, visible, toolsMissing, attempt]);
  const retry = () => {
    failures.current = 0;
    setCheck((value) => value + 1);
    setAttempt((value) => value + 1);
  };
  return <>
    {frame && <img alt={t("Screen of {name}", { name: device.name })} onLoad={() => {
      displayed.current = frame.dataUrl;
      acknowledge(frame.sequence);
    }} onError={() => {
      failures.current += 1;
      setFrame(null); setFailed(true); setReason(null);
      terminate.current();
    }} src={frame.dataUrl} />}
    {!frame && <p role="status">{failed ? t("Live video unavailable. Check that scrcpy and FFmpeg are installed and the device is authorized.") : t("Connecting to the screen…")}</p>}
    {failed && reason && <p className="small muted">{reason}</p>}
    {failed && !toolsMissing && <span className="small muted">{t("Trying again on its own…")}</span>}
    {failed && <button className="button small" onClick={retry} type="button">{t("Retry")}</button>}
    {failed && <ScreenToolsSetup key={check} onInstalled={retry} onMissing={setToolsMissing} />}
    {frame && <span className="small muted">{t("Live · up to 30 fps · view only")}</span>}
  </>;
}

function DeviceScreen({ device }: { device: ScreenDevice }) {
  const root = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(!document.hidden);
  useEffect(() => {
    let intersecting = true;
    const update = () => setVisible(intersecting && !document.hidden);
    document.addEventListener("visibilitychange", update);
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(([entry]) => {
      intersecting = !!entry?.isIntersecting;
      update();
    });
    if (root.current) observer?.observe(root.current);
    update();
    return () => { document.removeEventListener("visibilitychange", update); observer?.disconnect(); };
  }, []);
  return <div className="office-device-screen" ref={root}>
    {device.platform === "android" ? <LiveVideo device={device} visible={visible} /> : visible ? <DeviceSnapshots device={device} /> : null}
  </div>;
}

/** The same space can show the office or a local phone/emulator's screen. */
export function OfficeScreen({ children }: { children: ReactNode }) {
  const [view, setView] = useState<"crew" | "screen">("crew");
  const [devices, setDevices] = useState<ScreenDevice[]>([]);
  const [selected, setSelected] = useState("");
  const [unavailable, setUnavailable] = useState(false);
  const [loading, setLoading] = useState(true);
  const [rescan, setRescan] = useState(0);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const discover = async () => {
      if (stopped) return;
      if (!document.hidden) {
        try {
          const next = await api.screenDevices();
          if (stopped) return;
          setDevices(next);
          setUnavailable(false);
        } catch {
          if (stopped) return;
          setDevices([]);
          setUnavailable(true);
        }
        setLoading(false);
      }
      if (!stopped) timer = setTimeout(() => void discover(), 5000);
    };
    void discover();
    return () => { stopped = true; clearTimeout(timer); };
  }, [rescan]);
  const current = devices.find((device) => deviceKey(device) === selected) ?? devices[0];
  return (
    <div className="office-stage">
      <div aria-label={t("Panel view")} className="office-view-switch" role="group">
        <button aria-pressed={view === "crew"} className="button small" onClick={() => setView("crew")} type="button">{t("Crew")}</button>
        <button aria-pressed={view === "screen"} className="button small" onClick={() => setView("screen")} type="button">{t("Screen")}{devices.length > 0 ? ` (${devices.length})` : ""}</button>
      </div>
      {view === "crew" ? children : (
        <div className="office-device-view">
          {current ? <>
            <label className="small" htmlFor="office-screen-device">{t("Phone or emulator")}</label>
            <select className="select" id="office-screen-device" onChange={(event) => setSelected(event.target.value)} value={deviceKey(current)}>
              {devices.map((device) => <option key={deviceKey(device)} value={deviceKey(device)}>{device.name} · {device.platform === "android" ? "Android" : "iOS Simulator"} · {device.id}</option>)}
            </select>
            <DeviceScreen device={current} key={deviceKey(current)} />
          </> : <p className="small muted" role="status">{loading ? t("Looking for devices…") : unavailable ? t("Device preview unavailable. Use the desktop app with Android platform tools or Xcode simulators installed.") : t("No devices found. Connect an Android phone with USB debugging enabled, or start an Android emulator or iOS simulator.")}</p>}
          {!current && !loading && <ScreenToolsSetup key={rescan} onInstalled={() => setRescan((value) => value + 1)} />}
        </div>
      )}
    </div>
  );
}
