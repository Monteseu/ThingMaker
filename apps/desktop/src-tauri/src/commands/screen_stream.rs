//! Continuous scrcpy H.264 video, decoded once by FFmpeg and delivered with
//! one acknowledged frame in flight. No HTTP listener, video files or controls.
use super::{
    CommandResult,
    screen::{ScreenDevice, adb, output, valid_device},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{State, ipc::Channel};
use thingmaker_supervisor::DesktopError;
use tokio::{
    io::AsyncReadExt,
    net::TcpStream,
    process::{Child, Command},
    sync::watch,
};

const MAX_FRAME: usize = 4 * 1024 * 1024;
const LEASE: Duration = Duration::from_secs(25);

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScreenStreamEvent {
    #[serde(rename_all = "camelCase")]
    Frame {
        sequence: u64,
        data_url: String,
    },
    Error {
        message: String,
    },
}

struct Control {
    stop: watch::Sender<bool>,
    awaiting: AtomicU64,
    touched: Mutex<Instant>,
}
impl Control {
    fn touch(&self) {
        *self.touched.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
    fn expired(&self) -> bool {
        self.touched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
            > LEASE
    }
}
#[derive(Default)]
pub struct ScreenStreams {
    active: Arc<Mutex<HashMap<String, Arc<Control>>>>,
}

fn tool(name: &str) -> PathBuf {
    super::screen_tools::find(name).unwrap_or_else(|| {
        if cfg!(windows) {
            format!("{name}.exe").into()
        } else {
            name.into()
        }
    })
}

fn spawn(command: &mut Command) -> CommandResult<Child> {
    command.kill_on_drop(true).stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command.spawn().map_err(|_| {
        DesktopError::not_ready("Live video requires scrcpy and FFmpeg installed on this computer")
    })
}

/// Resources belong to the stream task, including throughout startup. Every
/// cancellation/error path closes the socket, kills/reaps children, removes
/// only our adb forwarding rule and deletes only our own temporary server jar.
struct AndroidVideo {
    device: ScreenDevice,
    remote: String,
    port: Option<u16>,
    socket_name: Option<String>,
    server: Option<Child>,
    decoder: Option<Child>,
}
impl AndroidVideo {
    fn new(device: ScreenDevice) -> Self {
        Self {
            device,
            remote: format!(
                "/data/local/tmp/thingmaker-{}.jar",
                uuid::Uuid::new_v4().simple()
            ),
            port: None,
            socket_name: None,
            server: None,
            decoder: None,
        }
    }
    async fn connect(&mut self) -> CommandResult<TcpStream> {
        let scrcpy = tool("scrcpy");
        let version = output(Command::new(&scrcpy).arg("--version")).await?;
        let version = String::from_utf8_lossy(&version)
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err(DesktopError::unsupported(
                "Cannot determine the installed scrcpy server version",
            ));
        }
        let parent = scrcpy.parent().unwrap_or(std::path::Path::new("."));
        // winget and Homebrew install a link; the jar sits beside the real file.
        let real = super::screen_tools::scrcpy_dir().unwrap_or_else(|| parent.to_path_buf());
        let jar = std::env::var_os("SCRCPY_SERVER_PATH")
            .map(PathBuf::from)
            .into_iter()
            .chain([
                parent.join("scrcpy-server"),
                parent.join("../share/scrcpy/scrcpy-server"),
                real.join("scrcpy-server"),
                real.join("../share/scrcpy/scrcpy-server"),
                PathBuf::from("/usr/share/scrcpy/scrcpy-server"),
            ])
            .find(|path| path.is_file())
            .ok_or_else(|| {
                DesktopError::not_ready("Cannot find scrcpy-server beside the installed scrcpy")
            })?;
        output(
            Command::new(adb())
                .args(["-s", &self.device.id, "push"])
                .arg(jar)
                .arg(&self.remote),
        )
        .await?;
        let scid = uuid::Uuid::new_v4().simple().to_string()[..7].to_string();
        self.socket_name = Some(format!("localabstract:scrcpy_{scid:0>8}"));
        let forward = output(Command::new(adb()).args([
            "-s",
            &self.device.id,
            "forward",
            "tcp:0",
            self.socket_name.as_ref().unwrap(),
        ]))
        .await?;
        self.port = Some(
            String::from_utf8_lossy(&forward)
                .trim()
                .parse::<u16>()
                .map_err(|_| DesktopError::io("Cannot open the device video tunnel"))?,
        );
        self.server = Some(spawn(
            Command::new(adb())
                .args([
                    "-s",
                    &self.device.id,
                    "shell",
                    &format!("CLASSPATH={}", self.remote),
                    "app_process",
                    "/",
                    "com.genymobile.scrcpy.Server",
                    &version,
                    &format!("scid={scid}"),
                    "tunnel_forward=true",
                    "audio=false",
                    "control=false",
                    "raw_stream=true",
                    "max_size=1280",
                    "max_fps=30",
                    "video_bit_rate=4000000",
                    "power_on=false",
                    "cleanup=false",
                ])
                .stdout(Stdio::null()),
        )?);
        // adb accepts the TCP connection before the Android socket is ready;
        // peek for the first byte and retry closed tunnels without losing data.
        loop {
            if self
                .server
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|_| DesktopError::io("Device video stopped"))?
                .is_some()
            {
                return Err(DesktopError::not_ready(
                    "scrcpy could not capture this device. Check USB debugging authorization.",
                ));
            }
            if let Ok(socket) =
                TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port.unwrap())).await
            {
                socket.set_nodelay(true).ok();
                if let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_secs(2), socket.peek(&mut [0])).await
                    && n > 0
                {
                    return Ok(socket);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn play(
        &mut self,
        control: &Control,
        send: impl Fn(ScreenStreamEvent) -> bool,
    ) -> CommandResult<()> {
        let mut socket = tokio::time::timeout(Duration::from_secs(20), self.connect())
            .await
            .map_err(|_| DesktopError::not_ready("Timed out starting device video"))??;
        let mut decoder = spawn(
            Command::new(tool("ffmpeg"))
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-probesize",
                    "32",
                    "-analyzeduration",
                    "0",
                    "-flags",
                    "low_delay",
                    "-f",
                    "h264",
                    "-i",
                    "pipe:0",
                    "-an",
                    "-vf",
                    "scale=w=720:h=1200:force_original_aspect_ratio=decrease",
                    "-threads",
                    "2",
                    "-filter_threads",
                    "1",
                    "-fps_mode",
                    "passthrough",
                    "-c:v",
                    "mjpeg",
                    "-q:v",
                    "5",
                    "-f",
                    "image2pipe",
                    "-flush_packets",
                    "1",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped()),
        )?;
        let mut input = decoder.stdin.take().unwrap();
        let mut frames = decoder.stdout.take().unwrap();
        self.decoder = Some(decoder);
        let read = async {
            let mut parser = JpegFrames::default();
            let mut bytes = [0; 32 * 1024];
            let mut sequence = 0;
            loop {
                let n = frames
                    .read(&mut bytes)
                    .await
                    .map_err(|_| DesktopError::io("Video decoder stopped"))?;
                if n == 0 {
                    return Err(DesktopError::not_ready("Device video disconnected"));
                }
                for jpeg in parser.push(&bytes[..n])? {
                    sequence += 1;
                    // A slow renderer drops frames instead of accumulating delay.
                    if control
                        .awaiting
                        .compare_exchange(0, sequence, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                        && !send(ScreenStreamEvent::Frame {
                            sequence,
                            data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)),
                        })
                    {
                        return Ok(());
                    }
                }
            }
        };
        tokio::select! {
            result = read => result,
            _ = tokio::io::copy(&mut socket, &mut input) => Err(DesktopError::not_ready("Device video disconnected")),
        }
    }
    async fn close(&mut self) {
        for child in [&mut self.decoder, &mut self.server].into_iter().flatten() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        // Cancellation can arrive after adb created the tunnel but before
        // its assigned port was returned. Reconcile by our unique socket name.
        if self.port.is_none()
            && let Some(socket_name) = &self.socket_name
            && let Ok(bytes) = output(Command::new(adb()).args(["forward", "--list"])).await
        {
            self.port = String::from_utf8_lossy(&bytes).lines().find_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() == 3 && fields[0] == self.device.id && fields[2] == socket_name {
                    fields[1].strip_prefix("tcp:")?.parse().ok()
                } else {
                    None
                }
            });
        }
        if let Some(port) = self.port.take() {
            let _ = output(Command::new(adb()).args([
                "-s",
                &self.device.id,
                "forward",
                "--remove",
                &format!("tcp:{port}"),
            ]))
            .await;
        }
        let _ = output(Command::new(adb()).args([
            "-s",
            &self.device.id,
            "shell",
            "rm",
            "-f",
            &self.remote,
        ]))
        .await;
    }
}

#[derive(Default)]
struct JpegFrames {
    bytes: Vec<u8>,
    scanned: usize,
}
impl JpegFrames {
    fn push(&mut self, chunk: &[u8]) -> CommandResult<Vec<Vec<u8>>> {
        self.bytes.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(end) = self.bytes[self.scanned..]
            .windows(2)
            .position(|v| v == [0xff, 0xd9])
        {
            let end = self.scanned + end + 2;
            if end > MAX_FRAME || !self.bytes.starts_with(&[0xff, 0xd8]) {
                return Err(DesktopError::io("Invalid video frame"));
            }
            frames.push(self.bytes.drain(..end).collect());
            self.scanned = 0;
        }
        if self.bytes.len() > MAX_FRAME {
            return Err(DesktopError::io("Video frame exceeds preview limits"));
        }
        self.scanned = self.bytes.len().saturating_sub(1);
        Ok(frames)
    }
}

#[tauri::command]
pub fn screen_stream_start(
    device: ScreenDevice,
    on_event: Channel<ScreenStreamEvent>,
    state: State<'_, ScreenStreams>,
) -> CommandResult<String> {
    if !valid_device(&device) || device.platform != "android" {
        return Err(DesktopError::unsupported(
            "Live scrcpy video requires an Android device",
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let (stop, mut stopped) = watch::channel(false);
    let control = Arc::new(Control {
        stop,
        awaiting: AtomicU64::new(0),
        touched: Mutex::new(Instant::now()),
    });
    let active = Arc::clone(&state.active);
    {
        let mut slots = active.lock().unwrap_or_else(|e| e.into_inner());
        if slots.len() >= 4 {
            return Err(DesktopError::not_ready("Too many device previews are open"));
        }
        slots.insert(id.clone(), Arc::clone(&control));
    }
    let task_id = id.clone();
    tauri::async_runtime::spawn(async move {
        let mut video = AndroidVideo::new(device);
        let lease = async {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if control.expired() {
                    break;
                }
            }
        };
        let result = tokio::select! {
            result = video.play(&control, |event| on_event.send(event).is_ok()) => result,
            _ = stopped.changed() => Ok(()),
            _ = lease => Ok(()),
        };
        if let Err(error) = result {
            let _ = on_event.send(ScreenStreamEvent::Error {
                message: error.message,
            });
        }
        video.close().await;
        active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&task_id);
    });
    Ok(id)
}
#[tauri::command]
pub fn screen_stream_stop(id: String, state: State<'_, ScreenStreams>) {
    if let Some(control) = state
        .active
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
    {
        let _ = control.stop.send(true);
    }
}
#[tauri::command]
pub fn screen_stream_ack(id: String, sequence: u64, state: State<'_, ScreenStreams>) {
    if let Some(control) = state
        .active
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
    {
        let _ = control
            .awaiting
            .compare_exchange(sequence, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}
#[tauri::command]
pub fn screen_stream_heartbeat(id: String, state: State<'_, ScreenStreams>) {
    if let Some(control) = state
        .active
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
    {
        control.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_multiple_frames_across_arbitrary_chunk_boundaries() {
        let mut parser = JpegFrames::default();
        assert!(parser.push(&[0xff]).unwrap().is_empty());
        assert!(parser.push(&[0xd8, 4, 0xff]).unwrap().is_empty());
        assert_eq!(
            parser.push(&[0xd9, 0xff, 0xd8, 5, 0xff, 0xd9]).unwrap(),
            vec![
                vec![0xff, 0xd8, 4, 0xff, 0xd9],
                vec![0xff, 0xd8, 5, 0xff, 0xd9]
            ]
        );
    }
    #[test]
    fn rejects_oversized_frames() {
        assert!(JpegFrames::default().push(&vec![0; MAX_FRAME + 1]).is_err());
    }
    #[test]
    fn stale_acknowledgements_cannot_release_a_new_frame_and_leases_expire() {
        let (stop, _) = watch::channel(false);
        let control = Control {
            stop,
            awaiting: AtomicU64::new(2),
            touched: Mutex::new(Instant::now() - Duration::from_secs(30)),
        };
        assert!(
            control
                .awaiting
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        );
        assert_eq!(control.awaiting.load(Ordering::Relaxed), 2);
        assert!(control.expired());
        control.touch();
        assert!(!control.expired());
    }
    #[tokio::test]
    #[ignore = "requires scrcpy, FFmpeg and an authorized Android device"]
    async fn cancelled_stream_reaps_children_and_removes_its_tunnel_and_server_file() {
        let device = super::super::screen::screen_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.platform == "android")
            .expect("connect Android first");
        let (stop, _) = watch::channel(false);
        let control = Control {
            stop,
            awaiting: AtomicU64::new(0),
            touched: Mutex::new(Instant::now()),
        };
        let mut video = AndroidVideo::new(device);
        let result =
            tokio::time::timeout(Duration::from_millis(1500), video.play(&control, |_| true)).await;
        video.close().await;
        assert!(
            result.is_err(),
            "the live stream should still be running at cancellation"
        );
        for child in [&mut video.server, &mut video.decoder]
            .into_iter()
            .flatten()
        {
            assert!(child.try_wait().unwrap().is_some());
        }
        let forwards = output(Command::new(adb()).args(["forward", "--list"]))
            .await
            .unwrap();
        if let Some(name) = &video.socket_name {
            assert!(!String::from_utf8_lossy(&forwards).contains(name));
        }
        output(Command::new(adb()).args([
            "-s",
            &video.device.id,
            "shell",
            "test",
            "!",
            "-e",
            &video.remote,
        ]))
        .await
        .unwrap();
    }
    #[tokio::test]
    #[ignore = "requires scrcpy, FFmpeg and an authorized Android device with a moving screen"]
    async fn streams_real_android_video_and_cleans_up() {
        let device = super::super::screen::screen_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.platform == "android")
            .expect("connect Android first");
        let (stop, _) = watch::channel(false);
        let control = Control {
            stop,
            awaiting: AtomicU64::new(0),
            touched: Mutex::new(Instant::now()),
        };
        let mut video = AndroidVideo::new(device);
        let count = AtomicU64::new(0);
        let started = Instant::now();
        let first = Mutex::new(None);
        let hashes = Mutex::new(std::collections::HashSet::new());
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            video.play(&control, |event| {
                if let ScreenStreamEvent::Frame { data_url, .. } = event {
                    let bytes = STANDARD
                        .decode(data_url.strip_prefix("data:image/jpeg;base64,").unwrap())
                        .unwrap();
                    assert!(image::load_from_memory(&bytes).is_ok());
                    first.lock().unwrap().get_or_insert(started.elapsed());
                    hashes.lock().unwrap().insert(blake3::hash(&bytes));
                    control.awaiting.store(0, Ordering::Release);
                    return count.fetch_add(1, Ordering::Relaxed) < 29;
                }
                true
            }),
        )
        .await;
        let port = video.port;
        video.close().await;
        assert!(
            result.is_ok(),
            "Timed out after {} decoded frames",
            count.load(Ordering::Relaxed)
        );
        assert!(result.unwrap().is_ok());
        assert!(count.load(Ordering::Relaxed) >= 30);
        assert!(
            hashes.lock().unwrap().len() > 1,
            "the test requires a moving screen"
        );
        eprintln!(
            "First frame: {:?}; 30 frames in {:?}",
            first.lock().unwrap(),
            started.elapsed()
        );
        assert!(video.server.as_mut().unwrap().try_wait().unwrap().is_some());
        assert!(
            video
                .decoder
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_some()
        );
        let forwards = output(Command::new(adb()).args(["forward", "--list"]))
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&forwards).contains(&format!("tcp:{}", port.unwrap())));
    }
}
