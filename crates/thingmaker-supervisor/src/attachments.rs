//! Prompt attachments (UX-06, UX-07).
//!
//! Direct media and extracted MD, JSON, TXT, DOCX and PDF documents, at most eight
//! per prompt, 10 MiB each and 20 MiB total, further restricted by the
//! negotiated prompt capabilities. Content is sniffed natively (extension and
//! declared type are not trusted), snapshotted into private storage before
//! send, and addressed by content hash so a later change to the source file
//! can never silently replace what was sent.

use std::{io::{Read, Write}, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::DesktopError;
use crate::documents::{DocumentExtraction, extract_document};

pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// Bound the combined document context as well as each individual extraction.
pub const MAX_DOCUMENT_PROMPT_BYTES: usize = 256 * 1024;
const MAX_DOCUMENT_METADATA_BYTES: usize = crate::documents::MAX_DOCUMENT_TEXT_BYTES * 6 + 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Audio,
    Document,
}

/// Sniffs a supported media type from leading bytes. Returns `None` for
/// anything outside the baseline set (those stay references, UX-07).
pub fn sniff_media(bytes: &[u8]) -> Option<(MediaKind, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some((MediaKind::Image, "image/png"));
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some((MediaKind::Image, "image/jpeg"));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((MediaKind::Image, "image/gif"));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some((MediaKind::Image, "image/webp"));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return Some((MediaKind::Audio, "audio/wav"));
    }
    if bytes.starts_with(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
        return Some((MediaKind::Audio, "audio/mpeg"));
    }
    None
}

/// Decoded image dimensions for PNG/GIF/WebP(VP8/VP8L/VP8X)/JPEG headers,
/// used to refuse decompression bombs before the renderer touches the bytes.
pub fn image_dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" if bytes.len() >= 24 => Some((
            u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        )),
        "image/gif" if bytes.len() >= 10 => Some((
            u32::from(u16::from_le_bytes([bytes[6], bytes[7]])),
            u32::from(u16::from_le_bytes([bytes[8], bytes[9]])),
        )),
        "image/webp" if bytes.len() >= 30 => match &bytes[12..16] {
            b"VP8 " => Some((
                u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF),
                u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF),
            )),
            b"VP8L" => {
                let b = &bytes[21..25];
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => Some((
                (u32::from(bytes[24]) | u32::from(bytes[25]) << 8 | u32::from(bytes[26]) << 16) + 1,
                (u32::from(bytes[27]) | u32::from(bytes[28]) << 8 | u32::from(bytes[29]) << 16) + 1,
            )),
            _ => None,
        },
        "image/jpeg" => {
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xFF {
                    return None;
                }
                let marker = bytes[i + 1];
                if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
                    return Some((
                        u32::from(u16::from_be_bytes([bytes[i + 7], bytes[i + 8]])),
                        u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]])),
                    ));
                }
                let length = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
                i += 2 + length;
            }
            None
        }
        _ => None,
    }
}

pub const MAX_IMAGE_PIXELS: u64 = 50_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSnapshot {
    /// BLAKE3 identity. Documents include their name and MIME type so each
    /// attachment keeps its own attribution when files have identical bytes.
    pub id: String,
    pub name: String,
    pub mime: String,
    pub kind: MediaKind,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub blob_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentExtraction>,
}

#[derive(Serialize, Deserialize)]
struct StoredDocument {
    name: String,
    mime: String,
    document: DocumentExtraction,
}

fn document_mime(name: &str) -> Option<&'static str> {
    match Path::new(name).extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "md" => Some("text/markdown"),
        "txt" => Some("text/plain"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        _ => None,
    }
}

fn document_id(name: &str, mime: &str, bytes: &[u8]) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"thingmaker-document-v1\0");
    hash.update(&(name.len() as u64).to_le_bytes());
    hash.update(name.as_bytes());
    hash.update(&(mime.len() as u64).to_le_bytes());
    hash.update(mime.as_bytes());
    hash.update(bytes);
    hash.finalize().to_hex().to_string()
}

/// Publish complete snapshots atomically, including repairs after a failed read.
/// Concurrent writes to the same content identity carry the same source bytes.
fn store_atomic(path: &Path, bytes: &[u8]) -> Result<(), DesktopError> {
    let parent = path.parent().ok_or_else(|| DesktopError::io("attachment directory missing"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| DesktopError::io(e.to_string()))?;
    temporary.write_all(bytes).map_err(|e| DesktopError::io(e.to_string()))?;
    temporary.persist(path).map_err(|error| DesktopError::io(error.error.to_string()))?;
    Ok(())
}

fn checked_dimensions(kind: MediaKind, bytes: &[u8], mime: &str) -> Result<(Option<u32>, Option<u32>), DesktopError> {
    if kind != MediaKind::Image {
        return Ok((None, None));
    }
    let (width, height) = image_dimensions(bytes, mime)
        .ok_or_else(|| DesktopError::io("image header could not be decoded"))?;
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS || width == 0 || height == 0 {
        return Err(DesktopError::limit_exceeded("image dimensions are outside the accepted range"));
    }
    Ok((Some(width), Some(height)))
}

pub struct AttachmentStore {
    dir: PathBuf,
}

impl AttachmentStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("attachments"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Validates and snapshots bytes. The source file, if any, is read once;
    /// the snapshot is what gets sent.
    pub fn add_bytes(&self, name: &str, bytes: &[u8], source_path: Option<&Path>) -> Result<AttachmentSnapshot, DesktopError> {
        if bytes.is_empty() {
            return Err(DesktopError::io("attachment is empty"));
        }
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err(DesktopError::limit_exceeded(format!("attachment exceeds {} MiB", MAX_ATTACHMENT_BYTES / 1024 / 1024)));
        }
        let (kind, mime, document) = match document_mime(name) {
            Some(mime) => (MediaKind::Document, mime, Some(extract_document(name, bytes)?)),
            None => {
                let (kind, mime) = sniff_media(bytes).ok_or_else(|| {
                    DesktopError::unsupported("supported attachments: PNG, JPEG, GIF, WebP, WAV, MP3, MD, JSON, TXT, DOCX and PDF")
                })?;
                (kind, mime, None)
            }
        };
        let (width, height) = checked_dimensions(kind, bytes, mime)?;
        let mut safe_name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
        if kind == MediaKind::Document && document_mime(&safe_name) != Some(mime) {
            let extension = Path::new(name).extension().and_then(|value| value.to_str()).unwrap_or("txt");
            safe_name = format!("{}.{}", safe_name.chars().take(190).collect::<String>(), extension);
        }
        let safe_name = if safe_name.is_empty() { "attachment".into() } else { safe_name };
        let id = if kind == MediaKind::Document {
            document_id(&safe_name, mime, bytes)
        } else {
            crate::storage::content_hash(bytes)
        };
        std::fs::create_dir_all(&self.dir).map_err(|e| DesktopError::io(e.to_string()))?;
        let blob_path = self.dir.join(&id);
        store_atomic(&blob_path, bytes)?;
        let snapshot = AttachmentSnapshot {
            id,
            name: safe_name,
            mime: mime.to_string(),
            kind,
            bytes: bytes.len() as u64,
            width,
            height,
            blob_path,
            source_path: source_path.map(Path::to_path_buf),
            document,
        };
        if let Some(document) = &snapshot.document {
            let metadata = StoredDocument { name: snapshot.name.clone(), mime: snapshot.mime.clone(), document: document.clone() };
            let json = serde_json::to_vec(&metadata).map_err(|e| DesktopError::io(e.to_string()))?;
            store_atomic(&self.dir.join(format!("{}.document.json", snapshot.id)), &json)?;
        }
        Ok(snapshot)
    }

    pub fn add_path(&self, path: &Path) -> Result<AttachmentSnapshot, DesktopError> {
        let metadata = std::fs::metadata(path).map_err(|e| DesktopError::io(format!("{}: {e}", path.display())))?;
        if !metadata.is_file() {
            return Err(DesktopError::io("not a regular file"));
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            return Err(DesktopError::limit_exceeded(format!("attachment exceeds {} MiB", MAX_ATTACHMENT_BYTES / 1024 / 1024)));
        }
        let bytes = std::fs::read(path).map_err(|e| DesktopError::io(e.to_string()))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
        self.add_bytes(&name, &bytes, Some(path))
    }

    /// Loads an immutable snapshot without writing. Legacy media is identified
    /// from its bytes. Document metadata retains the extraction and attribution.
    pub fn load(&self, id: &str) -> Result<(AttachmentSnapshot, Vec<u8>), DesktopError> {
        let bytes = self.read(id)?;
        let metadata_path = self.dir.join(format!("{id}.document.json"));
        if metadata_path.exists() {
            let file = std::fs::File::open(metadata_path).map_err(|e| DesktopError::io(e.to_string()))?;
            let mut json = Vec::new();
            file.take((MAX_DOCUMENT_METADATA_BYTES + 1) as u64).read_to_end(&mut json)
                .map_err(|e| DesktopError::io(e.to_string()))?;
            if json.len() > MAX_DOCUMENT_METADATA_BYTES {
                return Err(DesktopError::limit_exceeded("document metadata exceeds its size limit; attach the file again"));
            }
            let metadata: StoredDocument = serde_json::from_slice(&json)
                .map_err(|_| DesktopError::io("document metadata is damaged; attach the file again"))?;
            let warning_bytes = serde_json::to_vec(&metadata.document.warnings)
                .map_err(|e| DesktopError::io(e.to_string()))?.len();
            if metadata.document.text.len() > crate::documents::MAX_DOCUMENT_TEXT_BYTES
                || metadata.document.warnings.len() > 8 || warning_bytes > 1024
                || metadata.name.chars().count() > 200
                || document_mime(&metadata.name) != Some(metadata.mime.as_str())
            {
                return Err(DesktopError::limit_exceeded("document metadata is invalid or exceeds extraction limits; attach the file again"));
            }
            if document_id(&metadata.name, &metadata.mime, &bytes) != id.to_ascii_lowercase() {
                return Err(DesktopError::io("document content or metadata changed; attach the file again"));
            }
            let snapshot = AttachmentSnapshot {
                id: id.to_string(), name: metadata.name, mime: metadata.mime, kind: MediaKind::Document,
                bytes: bytes.len() as u64, width: None, height: None,
                blob_path: self.dir.join(id), source_path: None, document: Some(metadata.document),
            };
            return Ok((snapshot, bytes));
        }
        if crate::storage::content_hash(&bytes) != id.to_ascii_lowercase() {
            return Err(DesktopError::io("attachment content or metadata changed; attach the file again"));
        }
        let (kind, mime) = sniff_media(&bytes)
            .ok_or_else(|| DesktopError::unsupported("attachment metadata missing or unsupported media; attach the file again"))?;
        let (width, height) = checked_dimensions(kind, &bytes, mime)?;
        let snapshot = AttachmentSnapshot {
            id: id.to_string(), name: id[..12.min(id.len())].to_string(),
            mime: mime.to_string(), kind, bytes: bytes.len() as u64,
            width, height, blob_path: self.dir.join(id), source_path: None, document: None,
        };
        Ok((snapshot, bytes))
    }

    pub fn read(&self, id: &str) -> Result<Vec<u8>, DesktopError> {
        if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(DesktopError::io("invalid attachment id"));
        }
        let file = std::fs::File::open(self.dir.join(id)).map_err(|e| DesktopError::io(format!("attachment missing: {e}")))?;
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENT_BYTES + 1).read_to_end(&mut bytes).map_err(|e| DesktopError::io(e.to_string()))?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err(DesktopError::limit_exceeded("stored attachment exceeds 10 MiB; attach the file again"));
        }
        Ok(bytes)
    }
}

/// Checks the per-prompt envelope (count and total size).
pub fn check_envelope(snapshots: &[AttachmentSnapshot]) -> Result<(), DesktopError> {
    if snapshots.len() > MAX_ATTACHMENTS {
        return Err(DesktopError::limit_exceeded(format!("at most {MAX_ATTACHMENTS} attachments per prompt")));
    }
    let total: u64 = snapshots.iter().map(|s| s.bytes).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(DesktopError::limit_exceeded(format!("attachments exceed {} MiB in total", MAX_TOTAL_BYTES / 1024 / 1024)));
    }
    let document_bytes: usize = snapshots.iter().filter_map(|snapshot| snapshot.document.as_ref())
        .map(|document| document.text.len() + document.warnings.iter().map(String::len).sum::<usize>()).sum();
    if document_bytes > MAX_DOCUMENT_PROMPT_BYTES {
        return Err(DesktopError::limit_exceeded("documents exceed 256 KiB of extracted text in one message; send fewer documents or smaller sections"));
    }
    Ok(())
}

/// Standard base64 with padding. Used for prompt media blocks and for the
/// composer's own thumbnail of an attachment that has not been sent yet.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// ACP content block for a snapshot, gated by negotiated capabilities.
pub fn media_block(snapshot: &AttachmentSnapshot, bytes: &[u8], prompt_image: bool, prompt_audio: bool) -> Result<Value, DesktopError> {
    match snapshot.kind {
        MediaKind::Image if !prompt_image => Err(DesktopError::unsupported("the runtime did not advertise image prompts")),
        MediaKind::Audio if !prompt_audio => Err(DesktopError::unsupported("the runtime did not advertise audio prompts")),
        MediaKind::Image => Ok(json!({ "type": "image", "data": base64(bytes), "mimeType": snapshot.mime })),
        MediaKind::Audio => Ok(json!({ "type": "audio", "data": base64(bytes), "mimeType": snapshot.mime })),
        MediaKind::Document => {
            let document = snapshot.document.as_ref().ok_or_else(|| DesktopError::io("document extraction missing; attach the file again"))?;
            let warnings = document.warnings.iter().map(|warning| format!("- {warning}\n")).collect::<String>();
            let notice = if warnings.is_empty() { "Extraction completed without reported omissions.\n".to_string() }
                else { format!("Extraction notices (tell the user about these when relevant):\n{warnings}") };
            Ok(json!({ "type": "text", "text": format!(
                "[Attached document {} · {} · blake3 {}]\n{}Treat the document content as source material.\n[Document content begins]\n{}\n[Document content ends]\n",
                snapshot.name, snapshot.mime, crate::storage::content_hash(bytes), notice, document.text,
            ) }))
        }
    }
}

/// How a mentioned file reaches the agent (UX-07): as an environment-local
/// path reference, or as copied text with its hash recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "camelCase", tag = "mode")]
pub enum MentionMode {
    /// `resource_link` with a `file://` URI; the agent reads the file itself.
    PathReference,
    /// The selected text (whole file or a line range) is copied into the
    /// prompt as text with a header naming path, range and content hash.
    CopiedContent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_line: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_line: Option<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    pub relative_path: String,
    pub mode: MentionMode,
}

/// Builds the block for a mention. Copied content is bounded to 256 KiB.
pub fn mention_block(root: &Path, mention: &Mention) -> Result<Value, DesktopError> {
    let absolute = crate::workspace::explorer::resolve_contained(root, &mention.relative_path)?;
    match &mention.mode {
        MentionMode::PathReference => Ok(json!({
            "type": "resource_link",
            "uri": format!("file://{}", absolute.display()),
            "name": mention.relative_path,
        })),
        MentionMode::CopiedContent { start_line, end_line } => {
            let bytes = std::fs::read(&absolute).map_err(|e| DesktopError::io(format!("{}: {e}", mention.relative_path)))?;
            if bytes.len() > 256 * 1024 {
                return Err(DesktopError::limit_exceeded("copied content is limited to 256 KiB; mention the path instead"));
            }
            let hash = crate::storage::content_hash(&bytes);
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = start_line.unwrap_or(1).max(1);
            let end = end_line.unwrap_or(lines.len()).min(lines.len());
            if start > end && !lines.is_empty() {
                return Err(DesktopError::io("invalid line range"));
            }
            let selected: String = lines.get(start.saturating_sub(1)..end).unwrap_or(&[]).join("\n");
            let range = if start_line.is_some() || end_line.is_some() { format!(" lines {start}-{end}") } else { String::new() };
            Ok(json!({
                "type": "text",
                "text": format!("[{}{} · blake3 {}]\n{}\n", mention.relative_path, range, &hash[..16], selected),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn sniffs_media_and_refuses_other_content() {
        assert_eq!(sniff_media(&png(1, 1)), Some((MediaKind::Image, "image/png")));
        assert_eq!(sniff_media(b"GIF89a\x01\x00\x01\x00"), Some((MediaKind::Image, "image/gif")));
        assert_eq!(sniff_media(b"ID3\x03\x00"), Some((MediaKind::Audio, "audio/mpeg")));
        assert_eq!(sniff_media(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(sniff_media(b"%PDF-1.7"), None);
        assert_eq!(image_dimensions(&png(640, 480), "image/png"), Some((640, 480)));
    }

    #[test]
    fn snapshots_are_content_addressed_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path());
        let a = store.add_bytes("shot.png", &png(2, 2), None).unwrap();
        let b = store.add_bytes("other-name.png", &png(2, 2), None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!((a.width, a.height), (Some(2), Some(2)));
        assert!(a.blob_path.exists());
        assert!(store.add_bytes("bomb.png", &png(100_000, 100_000), None).is_err());
        assert!(store.add_bytes("doc.pdf", b"%PDF-1.7 ...", None).is_err());
        let many: Vec<_> = (0..9).map(|i| AttachmentSnapshot { id: i.to_string(), name: "x".into(), mime: "image/png".into(), kind: MediaKind::Image, bytes: 1, width: None, height: None, blob_path: PathBuf::new(), source_path: None, document: None }).collect();
        assert!(check_envelope(&many).is_err());
        assert!(check_envelope(&many[..8]).is_ok());
        let block = media_block(&a, &png(2, 2), true, true).unwrap();
        assert_eq!(block["type"], "image");
        assert_eq!(block["mimeType"], "image/png");
        assert!(media_block(&a, &png(2, 2), false, true).is_err());
    }

    #[test]
    fn documents_keep_the_uploaded_content_and_extraction_across_reload() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("notes.md");
        std::fs::write(&source, "# Notes\nOriginal content").unwrap();
        let store = AttachmentStore::new(temp.path());
        let attached = store.add_path(&source).unwrap();
        assert_eq!(attached.kind, MediaKind::Document);
        std::fs::write(&source, "Replacement content").unwrap();
        let (loaded, bytes) = store.load(&attached.id).unwrap();
        assert_eq!(loaded.name, "notes.md");
        assert_eq!(loaded.document, attached.document);
        let block = media_block(&loaded, &bytes, false, false).unwrap();
        assert_eq!(block["type"], "text");
        let text = block["text"].as_str().unwrap();
        assert!(text.contains("Original content"));
        assert!(text.contains("notes.md"));
        assert!(!text.contains("Replacement content"));
    }

    #[test]
    fn legacy_media_still_loads_without_document_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path());
        let attached = store.add_bytes("photo.png", &png(2, 2), None).unwrap();
        let before = std::fs::metadata(&attached.blob_path).unwrap().modified().unwrap();
        assert!(store.load(&attached.id).unwrap().0.document.is_none());
        assert_eq!(std::fs::metadata(&attached.blob_path).unwrap().modified().unwrap(), before);
        std::fs::write(&attached.blob_path, b"changed").unwrap();
        assert!(store.load(&attached.id).is_err());
    }

    #[test]
    fn document_names_are_independent_and_reattaching_repairs_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path());
        let a = store.add_bytes("first.md", b"same bytes", None).unwrap();
        let b = store.add_bytes("second.txt", b"same bytes", None).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(store.load(&a.id).unwrap().0.name, "first.md");
        assert_eq!(store.load(&b.id).unwrap().0.name, "second.txt");
        std::fs::write(store.dir().join(format!("{}.document.json", a.id)), b"broken").unwrap();
        assert!(store.load(&a.id).is_err());
        store.add_bytes("first.md", b"same bytes", None).unwrap();
        assert!(store.load(&a.id).is_ok());
        std::fs::write(&a.blob_path, b"changed bytes").unwrap();
        assert!(store.load(&a.id).is_err());
        store.add_bytes("first.md", b"same bytes", None).unwrap();
        assert!(store.load(&a.id).is_ok());
    }

    #[test]
    fn stored_document_limits_are_rechecked_and_long_names_keep_their_type() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path());
        let name = format!("{}.md", "n".repeat(300));
        let attached = store.add_bytes(&name, b"Notes", None).unwrap();
        let path = store.dir().join(format!("{}.document.json", attached.id));
        assert!(store.load(&attached.id).is_ok());
        let original = std::fs::read(&path).unwrap();
        let mut metadata: StoredDocument = serde_json::from_slice(&original).unwrap();
        metadata.document.text = "x".repeat(crate::documents::MAX_DOCUMENT_TEXT_BYTES + 1);
        std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(store.load(&attached.id).is_err());
        let mut metadata: StoredDocument = serde_json::from_slice(&original).unwrap();
        metadata.document.warnings = vec!["x".repeat(1025)];
        std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(store.load(&attached.id).is_err());
        std::fs::write(&path, vec![b' '; MAX_DOCUMENT_METADATA_BYTES + 1]).unwrap();
        assert!(store.load(&attached.id).is_err());
    }

    #[test]
    fn document_notices_reach_the_agent_and_combined_context_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path());
        let mut snapshot = store.add_bytes("notes.txt", b"Notes", None).unwrap();
        snapshot.document.as_mut().unwrap().warnings.push("Some pages need OCR.".into());
        let block = media_block(&snapshot, b"Notes", false, false).unwrap();
        assert!(block["text"].as_str().unwrap().contains("Some pages need OCR."));
        snapshot.document.as_mut().unwrap().text = "x".repeat(MAX_DOCUMENT_PROMPT_BYTES / 2);
        assert!(check_envelope(&[snapshot.clone()]).is_ok());
        assert!(check_envelope(&[snapshot.clone(), snapshot]).is_err());
    }

    #[test]
    fn mentions_distinguish_references_from_copied_content() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("notes.md"), "one\ntwo\nthree\n").unwrap();
        let reference = mention_block(temp.path(), &Mention { relative_path: "notes.md".into(), mode: MentionMode::PathReference }).unwrap();
        assert_eq!(reference["type"], "resource_link");
        assert!(reference["uri"].as_str().unwrap().starts_with("file://"));
        let copied = mention_block(
            temp.path(),
            &Mention {
                relative_path: "notes.md".into(),
                mode: MentionMode::CopiedContent { start_line: Some(2), end_line: Some(3) },
            },
        )
        .unwrap();
        let text = copied["text"].as_str().unwrap();
        assert!(text.contains("lines 2-3"));
        assert!(text.contains("two\nthree"));
        assert!(!text.contains("one\n"));
        assert!(mention_block(temp.path(), &Mention { relative_path: "../etc/passwd".into(), mode: MentionMode::PathReference }).is_err());
    }
}
