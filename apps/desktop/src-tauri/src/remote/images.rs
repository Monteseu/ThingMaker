//! Pictures in what goes to a phone, made light.
//!
//! Tool output carries screenshots inline as base64, often a megabyte each; a
//! long session holds dozens, and its snapshot and history then weigh tens of
//! megabytes — more than a phone on mobile data can take before its line is
//! given up on. Each picture bigger than [`KEEP_BYTES`] goes to the phone as a
//! small JPEG thumbnail instead, marked with `thingmakerFull` so the phone can
//! ask for the original (`remote_image`) when the person taps it.
//!
//! An older app that knows nothing of the mark simply shows the thumbnail.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Base64 this long or shorter goes as it is.
const KEEP_BYTES: usize = 48 * 1024;
/// The thumbnail's longest side, in pixels.
const THUMB_SIDE: u32 = 480;
const THUMB_QUALITY: u8 = 70;
/// A JSON text this long may hide a whole tool result, pictures included.
const NESTED_JSON_BYTES: usize = 32 * 1024;
/// Originals kept for phones to ask for, by base64 size.
const CACHE_BYTES: usize = 256 * 1024 * 1024;

/// One picture: the original, and what the phone gets instead.
struct Entry {
    data: Arc<str>,
    mime: String,
    /// Base64 JPEG; none when the picture could not be decoded.
    thumb: Option<Arc<str>>,
}

/// Originals by id, oldest dropped first past [`CACHE_BYTES`].
#[derive(Default)]
pub struct ImageCache {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Arc<Entry>>,
    order: VecDeque<String>,
    bytes: usize,
}

impl ImageCache {
    /// The original a thumbnail stands for, as `{ mimeType, data }`.
    pub fn original(&self, id: &str) -> Option<Value> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.entries.get(id).map(|entry| json!({ "mimeType": entry.mime, "data": &*entry.data }))
    }

    fn entry(&self, data: &str, mime: &str) -> (String, Arc<Entry>) {
        let id = format!("{:x}", Sha256::digest(data.as_bytes()))[..32].to_string();
        if let Some(entry) = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entries.get(&id) {
            return (id, entry.clone());
        }
        // Made outside the lock: decoding a big picture takes a while.
        let entry = Arc::new(Entry { data: Arc::from(data), mime: mime.to_string(), thumb: thumbnail(data).map(Arc::from) });
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !inner.entries.contains_key(&id) {
            inner.bytes += entry.data.len();
            inner.entries.insert(id.clone(), entry.clone());
            inner.order.push_back(id.clone());
            while inner.bytes > CACHE_BYTES {
                let Some(old) = inner.order.pop_front() else { break };
                if let Some(gone) = inner.entries.remove(&old) {
                    inner.bytes -= gone.data.len();
                }
            }
        }
        (id, entry)
    }

    /// Replaces every big picture in `value` with its thumbnail; true when
    /// anything changed.
    pub fn lighten(&self, value: &mut Value) -> bool {
        match value {
            Value::Array(items) => items.iter_mut().fold(false, |changed, item| self.lighten(item) | changed),
            Value::Object(record) => {
                if record.get("type").and_then(Value::as_str) == Some("image") && self.lighten_image(record) {
                    return true;
                }
                record.values_mut().fold(false, |changed, child| self.lighten(child) | changed)
            }
            // Some providers put a whole tool result, pictures and all, in a text.
            Value::String(text) if text.len() > NESTED_JSON_BYTES && text.trim_start().starts_with(['{', '[']) => {
                let Ok(mut nested) = serde_json::from_str::<Value>(text) else { return false };
                if !self.lighten(&mut nested) {
                    return false;
                }
                *text = nested.to_string();
                true
            }
            _ => false,
        }
    }

    /// An ACP/MCP image block (`data` + `mimeType`) or an Anthropic one
    /// (`source.data` + `source.media_type`).
    fn lighten_image(&self, record: &mut Map<String, Value>) -> bool {
        if record.contains_key("thingmakerFull") {
            return false;
        }
        let (data, mime) = if let Some(data) = record.get("data").and_then(Value::as_str) {
            (data, record.get("mimeType").or_else(|| record.get("mime_type")).and_then(Value::as_str))
        } else if let Some(source) = record.get("source").and_then(Value::as_object) {
            match source.get("data").and_then(Value::as_str) {
                Some(data) => (data, source.get("media_type").and_then(Value::as_str)),
                None => return false,
            }
        } else {
            return false;
        };
        if data.len() <= KEEP_BYTES {
            return false;
        }
        let mime = mime.unwrap_or("image/png").to_string();
        let (id, entry) = self.entry(data, &mime);
        let full = json!({ "id": id, "bytes": entry.data.len() / 4 * 3, "mimeType": entry.mime });
        // Undecodable: no picture at all, only the way to the original.
        let thumb = entry.thumb.as_deref().map(|thumb| Value::String(thumb.to_string())).unwrap_or(Value::String(String::new()));
        if let Some(source) = record.get_mut("source").and_then(Value::as_object_mut).filter(|source| source.contains_key("data")) {
            source.insert("data".into(), thumb);
            source.insert("media_type".into(), "image/jpeg".into());
        } else {
            record.insert("data".into(), thumb);
            record.insert("mimeType".into(), "image/jpeg".into());
            record.remove("mime_type");
        }
        record.insert("thingmakerFull".into(), full);
        true
    }
}

/// A small JPEG of a base64 picture (bare or a data URL), as base64.
fn thumbnail(data: &str) -> Option<String> {
    let bare = data.trim().split_once(";base64,").map_or(data.trim(), |(_, rest)| rest);
    let bytes = STANDARD.decode(bare.split_whitespace().collect::<String>()).ok()?;
    let picture = image::load_from_memory(&bytes).ok()?;
    let small = if picture.width().max(picture.height()) > THUMB_SIDE { picture.thumbnail(THUMB_SIDE, THUMB_SIDE) } else { picture };
    let mut jpeg = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, THUMB_QUALITY).encode_image(&small.to_rgb8()).ok()?;
    Some(STANDARD.encode(jpeg.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy PNG, big enough to be lightened.
    fn big_png() -> String {
        let mut picture = image::RgbImage::new(1600, 1000);
        for (x, y, pixel) in picture.enumerate_pixels_mut() {
            let n = x.wrapping_mul(2_654_435_761).wrapping_add(y.wrapping_mul(40_503)) as u8;
            *pixel = image::Rgb([n, n.wrapping_mul(3), n.wrapping_mul(7)]);
        }
        let mut png = Cursor::new(Vec::new());
        picture.write_to(&mut png, image::ImageFormat::Png).unwrap();
        STANDARD.encode(png.into_inner())
    }

    #[test]
    fn a_big_picture_becomes_a_thumbnail_with_the_way_to_its_original() {
        let cache = ImageCache::default();
        let original = big_png();
        let mut value = json!({ "toolCalls": { "t1": { "content": [{ "type": "content", "content": { "type": "image", "data": original, "mimeType": "image/png" } }] } } });
        assert!(cache.lighten(&mut value));
        let block = &value["toolCalls"]["t1"]["content"][0]["content"];
        assert_eq!(block["mimeType"], "image/jpeg");
        let thumb = STANDARD.decode(block["data"].as_str().unwrap()).unwrap();
        assert!(thumb.len() < 100 * 1024, "thumbnail is {} bytes", thumb.len());
        let small = image::load_from_memory(&thumb).unwrap();
        assert_eq!(small.width().max(small.height()), THUMB_SIDE);
        let id = block["thingmakerFull"]["id"].as_str().unwrap();
        assert_eq!(block["thingmakerFull"]["mimeType"], "image/png");
        assert_eq!(cache.original(id).unwrap(), json!({ "mimeType": "image/png", "data": original }));
    }

    #[test]
    fn small_pictures_and_text_are_left_alone() {
        let cache = ImageCache::default();
        let mut value = json!({ "type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png", "text": "x".repeat(100_000) });
        let before = value.clone();
        assert!(!cache.lighten(&mut value));
        assert_eq!(value, before);
    }

    #[test]
    fn pictures_inside_a_json_text_and_anthropic_blocks_are_lightened() {
        let cache = ImageCache::default();
        let original = big_png();
        let inner = json!([{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": original } }]).to_string();
        let mut value = json!({ "rawOutput": [{ "type": "text", "text": inner }] });
        assert!(cache.lighten(&mut value));
        let nested: Value = serde_json::from_str(value["rawOutput"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(nested[0]["source"]["media_type"], "image/jpeg");
        assert!(nested[0]["thingmakerFull"]["id"].is_string());
        assert!(nested[0]["source"]["data"].as_str().unwrap().len() < original.len() / 4);
    }

    #[test]
    fn an_undecodable_picture_keeps_only_the_way_to_its_original() {
        let cache = ImageCache::default();
        let junk = "A".repeat(KEEP_BYTES + 10);
        let mut value = json!({ "type": "image", "data": junk, "mimeType": "image/heic" });
        assert!(cache.lighten(&mut value));
        assert_eq!(value["data"], "");
        assert_eq!(cache.original(value["thingmakerFull"]["id"].as_str().unwrap()).unwrap()["mimeType"], "image/heic");
    }

    #[test]
    fn the_cache_forgets_the_oldest_originals_first() {
        let cache = ImageCache::default();
        let a = "A".repeat(CACHE_BYTES / 2 + 1);
        let b = "B".repeat(CACHE_BYTES / 2 + 1);
        let (first, _) = cache.entry(&a, "image/png");
        let (second, _) = cache.entry(&b, "image/png");
        assert!(cache.original(&first).is_none());
        assert!(cache.original(&second).is_some());
    }
}
