import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ScreenDevice } from "@thingmaker/contracts";
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

/** Continuous scrcpy video: the native side sends a new frame only after
 * the preceding image has loaded, keeping slow views from building a delay. */
function LiveVideo({ device, visible }: { device: ScreenDevice; visible: boolean }) {
  const [frame, setFrame] = useState<{ dataUrl: string; sequence: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
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
        setFrame(event);
        setFailed(false);
      } else {
        stopped = true;
        setFrame(null);
        setFailed(true);
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
    }).catch(() => {
      if (!stopped) { setFrame(null); setFailed(true); }
    });
    return () => { stopped = true; stop(); };
  }, [device.id, device.platform, visible, attempt]);
  return <>
    {frame && <img alt={t("Screen of {name}", { name: device.name })} onLoad={() => {
      displayed.current = frame.dataUrl;
      acknowledge(frame.sequence);
    }} onError={() => {
      setFrame(null); setFailed(true);
      terminate.current();
    }} src={frame.dataUrl} />}
    {!frame && <p role="status">{failed ? t("Live video unavailable. Check that scrcpy and FFmpeg are installed and the device is authorized.") : t("Connecting to the screen…")}</p>}
    {failed && <button className="button small" onClick={() => setAttempt((value) => value + 1)} type="button">{t("Retry")}</button>}
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
  }, []);
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
        </div>
      )}
    </div>
  );
}
