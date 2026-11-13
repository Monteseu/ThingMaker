//! Read-only previews of locally connected Android devices and booted iOS simulators.
use super::CommandResult;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};
use thingmaker_supervisor::DesktopError;
use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenDevice {
    pub(super) id: String,
    name: String,
    pub(super) platform: String,
}

pub(super) fn adb() -> PathBuf {
    let executable = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(&home).join("Library/Android/sdk"));
        roots.push(PathBuf::from(home).join("Android/Sdk"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Android/Sdk"));
    }
    roots
        .into_iter()
        .map(|root| root.join("platform-tools").join(executable))
        .chain([
            PathBuf::from("/opt/homebrew/bin/adb"),
            PathBuf::from("/usr/local/bin/adb"),
        ])
        .find(|path| path.is_file())
        .or_else(|| super::screen_tools::find("adb"))
        // scrcpy's Windows release bundles adb beside it.
        .or_else(|| {
            super::screen_tools::scrcpy_dir()
                .map(|dir| dir.join(executable))
                .filter(|path| path.is_file())
        })
        .unwrap_or_else(|| PathBuf::from(executable))
}

pub(super) async fn output(command: &mut Command) -> CommandResult<Vec<u8>> {
    command.kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // No console window for each frame.
    let result = tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .map_err(|_| DesktopError::io("Device preview timed out"))?
        .map_err(|_| DesktopError::not_ready("Device tools are unavailable"))?;
    if !result.status.success() {
        return Err(DesktopError::not_ready(
            "Cannot read the device screen. Check that it is connected and authorized.",
        ));
    }
    Ok(result.stdout)
}

fn android_devices(bytes: &[u8]) -> Vec<ScreenDevice> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            if fields.next()? != "device" {
                return None;
            }
            let model = fields.find_map(|field| field.strip_prefix("model:"));
            Some(ScreenDevice {
                id: id.into(),
                name: model.unwrap_or(id).replace('_', " "),
                platform: "android".into(),
            })
        })
        .collect()
}

#[tauri::command]
pub async fn screen_devices() -> CommandResult<Vec<ScreenDevice>> {
    let android = output(Command::new(adb()).args(["devices", "-l"])).await;
    let mut available = android.is_ok();
    let mut devices = match android {
        Ok(bytes) => android_devices(&bytes),
        Err(_) => Vec::new(), // Android tools are optional; iOS can still work.
    };
    if cfg!(target_os = "macos")
        && let Ok(bytes) = output(
            Command::new("/usr/bin/xcrun").args(["simctl", "list", "devices", "booted", "--json"]),
        )
        .await
    {
        available = true;
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let Some(groups) = value["devices"].as_object()
        {
            for list in groups.values().filter_map(|v| v.as_array()) {
                for device in list {
                    if device["state"] != "Booted" {
                        continue;
                    }
                    if let (Some(id), Some(name)) =
                        (device["udid"].as_str(), device["name"].as_str())
                    {
                        devices.push(ScreenDevice {
                            id: id.into(),
                            name: name.into(),
                            platform: "ios".into(),
                        });
                    }
                }
            }
        }
    }
    if available {
        Ok(devices)
    } else {
        Err(DesktopError::not_ready(
            "Install Android platform tools or Xcode simulators to preview device screens",
        ))
    }
}

pub(super) fn valid_device(device: &ScreenDevice) -> bool {
    !device.id.is_empty()
        && device.id.len() <= 256
        && !device.id.starts_with('-')
        && device
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:[]".contains(&b))
        && (device.platform == "android"
            || (cfg!(target_os = "macos")
                && device.platform == "ios"
                && uuid::Uuid::parse_str(&device.id).is_ok()))
}

#[tauri::command]
pub async fn screen_capture(device: ScreenDevice) -> CommandResult<String> {
    if !valid_device(&device) {
        return Err(DesktopError::io("Invalid screen device"));
    }
    let bytes = if device.platform == "android" {
        output(Command::new(adb()).args(["-s", &device.id, "exec-out", "screencap", "-p"])).await?
    } else {
        // Some simctl versions treat '-' as a literal filename despite their
        // help advertising stdout. Use a private temporary directory instead.
        let directory =
            tempfile::tempdir().map_err(|_| DesktopError::io("Cannot prepare a screen capture"))?;
        let path = directory.path().join("screen.png");
        output(
            Command::new("/usr/bin/xcrun")
                .args(["simctl", "io", &device.id, "screenshot", "--type=png"])
                .arg(&path),
        )
        .await?;
        tokio::fs::read(&path)
            .await
            .map_err(|_| DesktopError::io("Cannot read the simulator screen"))?
    };
    // Keep IPC frames small and decoding off the async runtime's worker threads.
    tokio::task::spawn_blocking(move || {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
        reader.set_format(image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|_| DesktopError::io("Cannot decode the device screen"))?;
        let image = image.thumbnail(900, 1200).to_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
            .encode_image(&image)
            .map_err(|_| DesktopError::io("Cannot prepare the device screen"))?;
        Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
    })
    .await
    .map_err(|_| DesktopError::io("Device preview stopped"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires an authorized Android device or a booted iOS simulator"]
    async fn captures_a_connected_device() {
        let device = screen_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("connect a device first");
        let frame = screen_capture(device).await.unwrap();
        let jpeg = STANDARD
            .decode(frame.strip_prefix("data:image/jpeg;base64,").unwrap())
            .unwrap();
        let image = image::load_from_memory(&jpeg).unwrap();
        assert!(image.width() > 0 && image.width() <= 900);
        assert!(image.height() > 0 && image.height() <= 1200);
    }
    #[test]
    fn only_authorized_android_devices_are_previewed() {
        let devices = android_devices(b"List of devices attached\nemulator-5554 device product:sdk model:Pixel_9\nphone unauthorized\nlost offline\n192.168.1.8:5555 device model:Phone\n");
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "Pixel 9");
        assert_eq!(devices[1].id, "192.168.1.8:5555");
    }
    #[test]
    fn rejects_options_paths_and_unknown_platforms() {
        for (id, platform) in [
            ("-d", "android"),
            ("../../file", "android"),
            ("a b", "android"),
            ("booted", "ios"),
            ("phone", "other"),
        ] {
            assert!(!valid_device(&ScreenDevice {
                id: id.into(),
                name: "".into(),
                platform: platform.into()
            }));
        }
    }
}
