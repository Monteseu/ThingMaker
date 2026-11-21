//! Document text extraction with bounded output and explicit omission notices.
//! PDF APIs: https://docs.rs/lopdf/0.47.0/lopdf/struct.Document.html
//! XML security: https://docs.rs/roxmltree/0.21.2/roxmltree/struct.ParsingOptions.html
//! PDF loading bounds each decoded object/xref stream to 4 MiB. The aggregate
//! decoded non-image stream budget is checked after loading. Peak memory
//! during parsing also depends on the PDF object structure.
use crate::error::DesktopError;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
    path::Path,
};

pub const MAX_DOCUMENT_TEXT_BYTES: usize = 128 * 1024;
const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_EXPANDED: usize = 32 * 1024 * 1024;
const MAX_PART: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_PAGES: usize = 512;
// Includes JSON quotes, escaping, commas and array brackets. Eight documents
// consume at most 8 KiB of warning metadata, leaving room for snapshot fields.
const MAX_WARNING_JSON_BYTES: usize = 1024;
const MAX_WARNINGS: usize = 8;
const MAX_WARNING_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentExtraction {
    pub text: String,
    pub warnings: Vec<String>,
}

pub fn extract_document(name: &str, bytes: &[u8]) -> Result<DocumentExtraction, DesktopError> {
    if bytes.len() > MAX_INPUT {
        return Err(DesktopError::limit_exceeded(
            "Document exceeds the 16 MiB input limit.",
        ));
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut out = Output::default();
    match ext.as_str() {
        "md" | "txt" | "json" => {
            let text = decode_text(bytes)?;
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
            {
                return Err(DesktopError::unsupported(
                    "Document contains binary control characters; only tab, LF and CR controls are supported in text documents.",
                ));
            }
            if ext == "json" {
                // Validate the entire original, including anything beyond the output limit.
                serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|_| DesktopError::unsupported("Invalid JSON document."))?;
            }
            out.push(&text);
        }
        "docx" => extract_docx(bytes, &mut out)?,
        "pdf" => extract_pdf(bytes, &mut out)?,
        _ => {
            return Err(DesktopError::unsupported(
                "Unsupported document extension; use MD, TXT, JSON, DOCX or PDF.",
            ));
        }
    }
    if out.text.trim().is_empty() {
        out.warn("Document contains no extractable text.");
    }
    Ok(out.finish())
}

fn decode_text(bytes: &[u8]) -> Result<String, DesktopError> {
    let invalid = || {
        DesktopError::unsupported("Document must contain valid UTF-8 or BOM-marked UTF-16 text.")
    };
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little = bytes[0] == 0xff;
        if (bytes.len() - 2) % 2 != 0 {
            return Err(invalid());
        }
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|b| {
                if little {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| invalid())
    } else {
        String::from_utf8(
            bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(bytes)
                .to_vec(),
        )
        .map_err(|_| invalid())
    }
}

#[derive(Default)]
struct Output {
    text: String,
    bytes: usize,
    chars: usize,
    warnings: BTreeSet<String>,
}
impl Output {
    fn push(&mut self, text: &str) {
        self.bytes += text.len();
        self.chars += text.chars().count();
        // Once any character was omitted, later text must not fill the gap.
        if self.bytes - text.len() > self.text.len() {
            return;
        }
        let mut end = text.len().min(MAX_DOCUMENT_TEXT_BYTES - self.text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
    }
    fn warn(&mut self, warning: impl Into<String>) {
        self.warnings.insert(warning.into());
    }
    fn finish(mut self) -> DocumentExtraction {
        if self.bytes > self.text.len() {
            self.warn(format!("Text truncated: retained {} of {} UTF-8 bytes ({} of {} Unicode characters); {} bytes omitted. Output limit: {} bytes.", self.text.len(), self.bytes, self.text.chars().count(), self.chars, self.bytes - self.text.len(), MAX_DOCUMENT_TEXT_BYTES));
        }
        let mut candidates: Vec<_> = self.warnings.into_iter().collect();
        // Preserve truncation, OCR and the compact DOCX omission/revision
        // summaries before lower priority presentation warnings.
        candidates.sort_by_key(|w| {
            (
                !w.starts_with("Text truncated:"),
                !w.contains("OCR"),
                !w.starts_with("DOCX auxiliary XML parts omitted:"),
                !w.starts_with("DOCX revisions omitted:"),
                w.clone(),
            )
        });
        let mut warnings = Vec::new();
        let mut omitted = 0;
        for mut warning in candidates {
            if warning.len() > MAX_WARNING_BYTES {
                let mut end = MAX_WARNING_BYTES - 3;
                while !warning.is_char_boundary(end) {
                    end -= 1;
                }
                warning.truncate(end);
                warning.push_str("...");
            }
            warnings.push(warning);
            // Reserve one slot and 96 serialized bytes for an overflow notice.
            if warnings.len() >= MAX_WARNINGS
                || serde_json::to_vec(&warnings)
                    .expect("strings serialize")
                    .len()
                    > MAX_WARNING_JSON_BYTES - 96
            {
                warnings.pop();
                omitted += 1;
            }
        }
        if omitted > 0 {
            warnings.push(format!(
                "Additional extraction warnings omitted: {omitted}; review the source document."
            ));
        }
        DocumentExtraction {
            text: self.text,
            warnings,
        }
    }
}

fn extract_docx(bytes: &[u8], out: &mut Output) -> Result<(), DesktopError> {
    let invalid = || DesktopError::unsupported("Malformed, encrypted or unsupported DOCX package.");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if zip.len() > MAX_ENTRIES {
        return Err(DesktopError::limit_exceeded(
            "DOCX has too many ZIP entries.",
        ));
    }
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    let mut omitted_parts = 0usize;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|_| invalid())?;
        total = total.checked_add(entry.size()).ok_or_else(invalid)?;
        if total > MAX_EXPANDED as u64 {
            return Err(DesktopError::limit_exceeded(
                "DOCX exceeds the 32 MiB expanded resource limit.",
            ));
        }
        let name = entry.name().to_owned();
        if !names.insert(name.clone()) {
            return Err(invalid());
        }
        if name.starts_with("word/media/") {
            out.warn("DOCX images omitted; image text requires OCR.");
        }
        if name.starts_with("word/charts/") {
            out.warn("DOCX charts omitted, including chart labels and data.");
        }
        if name.starts_with("word/embeddings/") {
            out.warn("DOCX embedded objects omitted.");
        }
        if name.starts_with("word/") && name.ends_with(".xml") && name != "word/document.xml" {
            omitted_parts += 1;
        }
    }
    if omitted_parts > 0 {
        out.warn(format!("DOCX auxiliary XML parts omitted: {omitted_parts} (including headers, footers, notes, comments, styles or chart data when present); only main document text is extracted."));
    }
    let types_xml = read_xml_part(&mut zip, "[Content_Types].xml")?;
    let types = roxmltree::Document::parse_with_options(&types_xml, xml_options())
        .map_err(|_| invalid())?;
    let types_ns = "http://schemas.openxmlformats.org/package/2006/content-types";
    if !types.root_element().has_tag_name((types_ns, "Types")) || !types.root_element().children().any(|n| {
        n.has_tag_name((types_ns, "Override")) && n.attribute("PartName") == Some("/word/document.xml")
            && n.attribute("ContentType") == Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
    }) { return Err(invalid()); }
    let xml = read_xml_part(&mut zip, "word/document.xml")?;
    let doc =
        roxmltree::Document::parse_with_options(&xml, xml_options()).map_err(|_| invalid())?;
    let root = doc.root_element();
    if !word(root, "document") {
        return Err(invalid());
    }
    let body = root
        .children()
        .find(|n| word(*n, "body"))
        .ok_or_else(invalid)?;
    out.warn("DOCX formatting, numbering and pagination are not preserved; tables use tabs and newlines.");
    walk_docx(body, out, 0)
}

fn xml_options<'input>() -> roxmltree::ParsingOptions<'input> {
    roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 100_000,
        ..Default::default()
    }
}

fn read_xml_part(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<String, DesktopError> {
    let mut part = zip
        .by_name(name)
        .map_err(|_| DesktopError::unsupported("DOCX is missing a required readable XML part."))?;
    if part.size() > MAX_PART as u64 {
        return Err(DesktopError::limit_exceeded("DOCX XML part exceeds 4 MiB."));
    }
    let mut data = Vec::new();
    (&mut part)
        .take((MAX_PART + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|_| DesktopError::unsupported("Malformed or unsupported DOCX XML part."))?;
    if data.len() > MAX_PART {
        return Err(DesktopError::limit_exceeded("DOCX XML part exceeds 4 MiB."));
    }
    decode_text(&data)
}

fn word(node: roxmltree::Node<'_, '_>, local: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == local
        && matches!(
            node.tag_name().namespace(),
            Some(
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                    | "http://purl.oclc.org/ooxml/wordprocessingml/main"
            )
        )
}

fn walk_docx(
    node: roxmltree::Node<'_, '_>,
    out: &mut Output,
    depth: usize,
) -> Result<(), DesktopError> {
    if depth > 128 {
        return Err(DesktopError::limit_exceeded(
            "DOCX XML nesting exceeds 128 levels.",
        ));
    }
    if !node.is_element() {
        return Ok(());
    }
    if word(node, "del") || word(node, "moveFrom") {
        out.warn("DOCX revisions omitted: deleted text and move sources; inserted text and move destinations retained.");
        return Ok(());
    }
    if word(node, "ins") || word(node, "moveTo") {
        out.warn("DOCX revisions omitted: deleted text and move sources; inserted text and move destinations retained.");
    }
    let alternate = node.has_tag_name((
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
        "AlternateContent",
    ));
    if ["drawing", "pict", "object", "altChunk", "instrText"]
        .iter()
        .any(|name| word(node, name))
        || alternate
    {
        out.warn("DOCX content omitted: drawings, objects, alternate content or field instructions when present.");
        return Ok(());
    }
    if word(node, "t") {
        for child in node.children().filter(|n| n.is_text()) {
            out.push(child.text().unwrap_or(""));
        }
        return Ok(());
    }
    if word(node, "tab") {
        out.push("\t");
    }
    if word(node, "noBreakHyphen") {
        out.push("\u{2011}");
    }
    if word(node, "softHyphen") {
        out.push("\u{00ad}");
    }
    if word(node, "br") || word(node, "cr") {
        out.push("\n");
    }
    if word(node, "sym") {
        out.warn("DOCX symbol glyph omitted (font-dependent character).");
    }
    for child in node.children() {
        walk_docx(child, out, depth + 1)?;
    }
    if word(node, "p") {
        // A cell's final paragraph must not turn every column into a new row.
        // Separate multiple paragraphs inside the cell with a visible delimiter.
        if node.ancestors().any(|n| word(n, "tc")) {
            if node.next_siblings().skip(1).any(|n| word(n, "p")) {
                out.push(" | ");
            }
        } else {
            out.push("\n");
        }
    }
    if word(node, "tr") {
        out.push("\n");
    }
    if word(node, "tc") && node.next_siblings().skip(1).any(|n| word(n, "tc")) {
        out.push("\t");
    }
    Ok(())
}

fn extract_pdf(bytes: &[u8], out: &mut Output) -> Result<(), DesktopError> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(DesktopError::unsupported("Invalid PDF header."));
    }
    let doc = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            strict: true,
            max_decompressed_size: Some(MAX_PART),
            ..Default::default()
        },
    )
    .map_err(|_| {
        DesktopError::unsupported(
            "Malformed, encrypted or unsupported PDF (or decompression limit exceeded).",
        )
    })?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(DesktopError::unsupported(
            "Encrypted PDFs are not supported, including PDFs with an empty password.",
        ));
    }
    if doc.objects.len() > 50_000 {
        return Err(DesktopError::limit_exceeded("PDF has too many objects."));
    }
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err(DesktopError::unsupported("PDF contains no readable pages."));
    }
    if pages.len() > MAX_PAGES {
        return Err(DesktopError::limit_exceeded("PDF exceeds 512 pages."));
    }
    // This aggregate check is POST-LOAD. LoadOptions only supports a per-stream
    // decompression limit, so it cannot cap total allocations during loading.
    // Bound decoded non-image streams before text extraction. Image codecs are
    // deliberately never invoked; compressed image pixels cannot become text.
    let mut expanded = 0usize;
    for object in doc.objects.values() {
        if let lopdf::Object::Stream(stream) = object {
            if stream
                .dict
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .ok()
                == Some(b"Image".as_slice())
            {
                out.warn("PDF images omitted; text inside images requires OCR.");
                continue;
            }
            let plain = stream.get_plain_content_with_limit(MAX_PART).map_err(|_| {
                DesktopError::unsupported(
                    "PDF contains an unsupported, malformed or oversized compressed stream.",
                )
            })?;
            expanded = expanded.checked_add(plain.len()).ok_or_else(|| {
                DesktopError::limit_exceeded("PDF expanded resource limit exceeded.")
            })?;
            if expanded > MAX_EXPANDED {
                return Err(DesktopError::limit_exceeded(
                    "PDF exceeds the 32 MiB decoded non-image stream budget (checked after loading).",
                ));
            }
        }
    }
    out.warn("PDF reading order, spacing, tables and layout are approximate; font mappings may omit or misdecode characters. Annotations, interactive forms, text within Form XObjects and embedded files are not extracted.");
    out.warn(
        "PDF graphics, charts and inline images are omitted; text inside images requires OCR.",
    );
    let mut any_text = false;
    let mut failed_pages = BTreeSet::new();
    let mut empty_pages = BTreeSet::new();
    for (number, _) in pages {
        out.push(&format!("[Page {number}]\n"));
        // Chunks retain partial successes and expose failed text operations.
        let chunks = doc.extract_text_chunks_with_limit(&[number], MAX_PART);
        let mut page_text = false;
        for chunk in chunks {
            match chunk {
                Ok(text) => {
                    page_text |= !text.trim().is_empty();
                    out.push(&text);
                }
                Err(_) => {
                    failed_pages.insert(number);
                }
            }
        }
        if failed_pages.contains(&number) {
            out.push("\n[Extraction warning: some page text could not be decoded.]\n");
        }
        if !page_text {
            empty_pages.insert(number);
            out.push("[Extraction warning: no extractable page text; scanned pages need OCR.]\n");
        }
        any_text |= page_text;
        out.push("\n");
    }
    if !failed_pages.is_empty() {
        out.warn(format!("PDF text extraction failed on {} pages ({}); unsupported encoding or malformed content. Page markers identify omissions in extracted text.", failed_pages.len(), page_summary(&failed_pages)));
    }
    if !empty_pages.is_empty() {
        out.warn(format!("PDF has {} pages with no extractable text ({}); OCR is needed for scanned/image-only pages. Page markers identify omissions in extracted text.", empty_pages.len(), page_summary(&empty_pages)));
    }
    if !any_text {
        out.warn("PDF has no extractable text. It may be scanned; OCR is needed before its content can be read.");
    }
    Ok(())
}

fn page_summary(pages: &BTreeSet<u32>) -> String {
    let sample = pages
        .iter()
        .take(12)
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if pages.len() > 12 {
        format!("pages {sample},... (+{} more)", pages.len() - 12)
    } else {
        format!("pages {sample}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn documents_without_visible_text_report_empty_extraction() {
        let text = extract_document("blank.txt", b" \n\t").unwrap();
        assert!(
            text.warnings
                .iter()
                .any(|warning| warning.contains("no extractable text"))
        );
        let word = extract_document("blank.docx", &docx(&wrap("<w:p/>"), &[])).unwrap();
        assert!(
            word.warnings
                .iter()
                .any(|warning| warning.contains("no extractable text"))
        );
    }

    fn docx(xml: &str, extras: &[&str]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
        for (name, content) in [("[Content_Types].xml", types), ("word/document.xml", xml)]
            .into_iter()
            .chain(extras.iter().map(|n| (*n, "")))
        {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    fn wrap(body: &str) -> String {
        format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        )
    }
    #[test]
    fn documents_strict_encodings_and_json_preservation() {
        let original = " { \"x\": \"é🦀\" }\n";
        assert_eq!(
            extract_document("test.JSON", original.as_bytes())
                .unwrap()
                .text,
            original
        );
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in original.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_eq!(extract_document("x.json", &bytes).unwrap().text, original);
        }
        for bytes in [
            &[0xff][..],
            &[0xff, 0xfe, 0x00][..],
            &[0xff, 0xfe, 0x00, 0xd8][..],
        ] {
            assert!(extract_document("x.txt", bytes).is_err());
        }
        assert!(extract_document("x.json", b"{broken}").is_err());
        assert!(extract_document("x.exe", b"hi").is_err());
    }
    #[test]
    fn documents_unicode_truncation_counts_and_json_validation_after_limit() {
        let text = "🦀".repeat(MAX_DOCUMENT_TEXT_BYTES / 4 + 1);
        let result = extract_document("x.md", text.as_bytes()).unwrap();
        assert_eq!(result.text.len(), MAX_DOCUMENT_TEXT_BYTES);
        assert!(result.warnings[0].contains("4 bytes omitted"));
        let invalid = format!("{}!", " ".repeat(MAX_DOCUMENT_TEXT_BYTES));
        assert!(extract_document("x.json", invalid.as_bytes()).is_err());
        assert!(extract_document("x.txt", &vec![b'a'; MAX_INPUT + 1]).is_err());
    }
    #[test]
    fn documents_docx_entities_order_tables_and_omissions() {
        let xml = wrap(
            "<w:p><w:r><w:t>A &amp; &#x1F980;</w:t><w:tab/><w:t>B</w:t><w:br/><w:t>C</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>D</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>E</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
        );
        let result = extract_document(
            "x.docx",
            &docx(
                &xml,
                &[
                    "word/media/a.png",
                    "word/charts/c.xml",
                    "word/embeddings/a.bin",
                    "word/header1.xml",
                ],
            ),
        )
        .unwrap();
        assert_eq!(result.text, "A & 🦀\tB\nC\nD\tE\n");
        for needle in [
            "images omitted",
            "charts omitted",
            "embedded objects",
            "auxiliary XML parts omitted: 2",
        ] {
            assert!(
                result.warnings.iter().any(|w| w.contains(needle)),
                "{needle}"
            );
        }
    }
    #[test]
    fn documents_table_rows_empty_cells_and_multiple_paragraphs() {
        let xml = wrap(
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc><w:tc><w:p/></w:tc><w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>D</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>E</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
        );
        assert_eq!(
            extract_document("x.docx", &docx(&xml, &[])).unwrap().text,
            "A | B\t\tC\nD\tE\n"
        );
    }
    #[test]
    fn documents_revisions_and_special_hyphens_in_both_word_namespaces() {
        let body = "<w:p><w:moveFrom><w:r><w:t>moved</w:t></w:r></w:moveFrom><w:moveTo><w:r><w:t>moved</w:t></w:r></w:moveTo><w:del><w:r><w:delText>deleted</w:delText></w:r></w:del><w:ins><w:r><w:t>new</w:t><w:noBreakHyphen/><w:t>word</w:t><w:softHyphen/><w:t>end</w:t></w:r></w:ins></w:p>";
        for ns in [
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
            "http://purl.oclc.org/ooxml/wordprocessingml/main",
        ] {
            let xml = format!(r#"<w:document xmlns:w="{ns}"><w:body>{body}</w:body></w:document>"#);
            let result = extract_document("x.docx", &docx(&xml, &[])).unwrap();
            assert_eq!(result.text, "movednew\u{2011}word\u{00ad}end\n");
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|w| w.contains("revisions omitted:") && w.contains("move sources"))
            );
            assert_warning_budget(&result);
        }
    }
    #[test]
    fn documents_word_tag_checks_do_not_match_foreign_namespaces() {
        let body = r#"<w:p xmlns:f="urn:foreign" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><f:del><w:r><w:t>A</w:t></w:r></f:del><f:moveFrom><w:r><w:t>B</w:t></w:r></f:moveFrom><f:drawing><w:r><w:t>C</w:t></w:r></f:drawing><f:AlternateContent><w:r><w:t>D</w:t></w:r></f:AlternateContent><w:r><f:noBreakHyphen/><f:softHyphen/><f:t>foreign text</f:t><w:t>E</w:t></w:r><mc:AlternateContent><mc:Choice><w:r><w:t>omitted alternate</w:t></w:r></mc:Choice></mc:AlternateContent></w:p>"#;
        for ns in [
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
            "http://purl.oclc.org/ooxml/wordprocessingml/main",
        ] {
            let xml = format!(r#"<w:document xmlns:w="{ns}"><w:body>{body}</w:body></w:document>"#);
            let result = extract_document("x.docx", &docx(&xml, &[])).unwrap();
            assert_eq!(result.text, "ABCDE\n");
            assert!(
                !result
                    .warnings
                    .iter()
                    .any(|w| w.contains("revisions omitted:"))
            );
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|w| w.contains("alternate content"))
            );
        }
        let foreign = r#"<f:document xmlns:f="urn:foreign"><f:body/></f:document>"#;
        assert!(extract_document("x.docx", &docx(foreign, &[])).is_err());
    }
    #[test]
    fn documents_reject_binary_controls_but_preserve_text_whitespace() {
        for bytes in [
            &b"hello\0world"[..],
            &b"\x1b[31mhello"[..],
            &b"\x7f"[..],
            &[0xff, 0xfe, 0, 0][..],
        ] {
            assert!(extract_document("x.txt", bytes).is_err());
        }
        let text = "one\ttwo\r\nthree\n";
        assert_eq!(
            extract_document("x.txt", text.as_bytes()).unwrap().text,
            text
        );
        assert!(extract_document("x.json", br#"{"control":"\u0000"}"#).is_ok());
    }
    fn assert_warning_budget(result: &DocumentExtraction) {
        assert!(result.warnings.len() <= MAX_WARNINGS);
        assert!(result.warnings.iter().all(|w| w.len() <= MAX_WARNING_BYTES));
        assert!(serde_json::to_vec(&result.warnings).unwrap().len() <= MAX_WARNING_JSON_BYTES);
    }
    #[test]
    fn documents_serde_round_trip_and_warning_overflow_budget() {
        let mut out = Output::default();
        out.push(&"x".repeat(MAX_DOCUMENT_TEXT_BYTES + 1));
        out.warn("OCR is needed for scanned content.");
        for i in 0..100 {
            out.warn(format!("Warning {i}: {}", "🦀\n\"".repeat(100)));
        }
        let result = out.finish();
        assert_warning_budget(&result);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.starts_with("Text truncated:"))
        );
        assert!(result.warnings.iter().any(|w| w.contains("OCR")));
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("Additional extraction warnings omitted:"))
        );
        assert_eq!(
            serde_json::from_slice::<DocumentExtraction>(&serde_json::to_vec(&result).unwrap())
                .unwrap(),
            result
        );
    }
    #[test]
    fn documents_many_auxiliary_parts_have_one_compact_warning() {
        let names = (0..MAX_ENTRIES - 2)
            .map(|i| format!("word/header{i}{}.xml", "x".repeat(100)))
            .collect::<Vec<_>>();
        let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let result = extract_document("x.docx", &docx(&wrap("<w:p/>"), &refs)).unwrap();
        assert_warning_budget(&result);
        assert_eq!(
            result
                .warnings
                .iter()
                .filter(|w| w.contains("auxiliary XML parts omitted:"))
                .count(),
            1
        );
        assert!(result.warnings.iter().any(|w| w.contains("4094")));
    }
    #[test]
    fn documents_omission_summaries_survive_warning_pressure() {
        let body = "<w:moveFrom><w:r><w:t>old</w:t></w:r></w:moveFrom><w:moveTo><w:p><w:r><w:t>new</w:t><w:sym/></w:r><w:drawing/></w:p></w:moveTo>";
        let long_text = format!(
            "{body}<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            "x".repeat(MAX_DOCUMENT_TEXT_BYTES + 1)
        );
        let result = extract_document(
            "x.docx",
            &docx(
                &wrap(&long_text),
                &[
                    "word/header1.xml",
                    "word/media/a.png",
                    "word/charts/chart1.xml",
                    "word/embeddings/file.bin",
                ],
            ),
        )
        .unwrap();
        assert_warning_budget(&result);
        for required in [
            "Text truncated:",
            "DOCX auxiliary XML parts omitted: 2",
            "DOCX revisions omitted:",
        ] {
            assert!(
                result.warnings.iter().any(|w| w.starts_with(required)),
                "{required}"
            );
        }
        assert!(result.warnings.iter().any(|w| w.contains("OCR")));
    }
    #[test]
    fn documents_malformed_docx_and_dtd_rejected() {
        assert!(extract_document("x.docx", b"not zip").is_err());
        for xml in ["<broken>", "<!DOCTYPE a [<!ENTITY x 'secret'>]><a>&x;</a>"] {
            assert!(extract_document("x.docx", &docx(xml, &[])).is_err());
        }
        let deep = wrap(&format!("{}{}", "<w:p>".repeat(130), "</w:p>".repeat(130)));
        assert!(extract_document("x.docx", &docx(&deep, &[])).is_err());
    }
    #[test]
    fn documents_docx_expansion_limit_and_unicode_chunk_prefix() {
        let xml = wrap(&format!(
            "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            "a".repeat(MAX_PART)
        ));
        assert!(extract_document("x.docx", &docx(&xml, &[])).is_err());
        let mut out = Output::default();
        out.push(&"a".repeat(MAX_DOCUMENT_TEXT_BYTES - 1));
        out.push("🦀");
        out.push("z");
        let result = out.finish();
        assert_eq!(result.text.len(), MAX_DOCUMENT_TEXT_BYTES - 1);
        assert!(result.warnings[0].contains("5 bytes omitted"));
    }
    fn pdf(text: bool) -> Vec<u8> {
        pdf_pages(text, 1)
    }
    fn pdf_pages(text: bool, count: usize) -> Vec<u8> {
        use lopdf::{Object, Stream, dictionary};
        let mut doc = lopdf::Document::with_version("1.5");
        let pages = doc.new_object_id();
        let font = doc.add_object(
            dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
        );
        let content = doc.add_object(Stream::new(
            dictionary! {},
            if text {
                b"BT /F1 12 Tf (Hello PDF) Tj ET".to_vec()
            } else {
                vec![]
            },
        ));
        let mut kids = Vec::<Object>::new();
        for _ in 0..count {
            let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages, "Contents" => content,
                "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()]});
            kids.push(page.into());
        }
        doc.objects.insert(
            pages,
            Object::Dictionary(
                dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count as i64},
            ),
        );
        let root = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", root);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn documents_pdf_text_and_ocr_warning() {
        let text = extract_document("x.pdf", &pdf(true)).unwrap();
        assert!(text.text.contains("[Page 1]\n"));
        assert!(text.text.contains("Hello PDF"));
        let empty = extract_document("x.pdf", &pdf(false)).unwrap();
        assert!(empty.warnings.iter().any(|w| w.contains("OCR is needed")));
        assert!(extract_document("x.pdf", b"%PDF-malformed").is_err());
    }
    #[test]
    fn documents_pdf_page_warnings_are_aggregated_with_local_markers() {
        let result = extract_document("x.pdf", &pdf_pages(false, MAX_PAGES)).unwrap();
        assert_warning_budget(&result);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("512 pages with no extractable text"))
        );
        assert!(result.text.contains("[Page 512]\n[Extraction warning:"));
        assert!(result.warnings.iter().any(|w| w.contains("OCR is needed")));
    }
    #[test]
    fn documents_pdf_decoded_stream_limits_are_post_load_and_per_stream() {
        use crate::error::ErrorCode;
        use lopdf::{Stream, dictionary};
        // Highly compressible unused streams exercise pre-extraction resource
        // validation without depending on text output or reaching input limits.
        for (count, size, expected) in [
            (1, MAX_PART + 1, ErrorCode::Unsupported),
            (9, MAX_PART, ErrorCode::LimitExceeded),
        ] {
            let mut doc = lopdf::Document::load_mem(&pdf(true)).unwrap();
            for _ in 0..count {
                let mut stream = Stream::new(dictionary! {}, vec![b'x'; size]);
                stream.compress().unwrap();
                assert!(stream.content.len() < size);
                doc.add_object(stream);
            }
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            assert!(bytes.len() < MAX_INPUT);
            let error = extract_document("x.pdf", &bytes).unwrap_err();
            assert_eq!(error.code, expected);
            if count > 1 {
                assert!(error.message.contains("checked after loading"));
            }
        }
    }
}
