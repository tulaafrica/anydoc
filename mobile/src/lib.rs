//! C FFI for the Tula document pipeline: bytes in, one buffer out.
//!
//! Contract, mirroring rn-docx-ir's `convertDocxToIr`: this NEVER panics
//! across the boundary and never returns a partial result. Every input -
//! corrupt zip, encrypted OLE, a bug in a parser - produces either a
//! complete `{"status":"ok",...}` payload or a clean
//! `{"status":"fallback","reason":...,"detail":...}`, so the JS side needs
//! no defence beyond JSON.parse.
//!
//! Output buffer layout (one allocation, one bridge crossing):
//!
//! ```text
//! [4 bytes LE: json length][json UTF-8][asset bytes, concatenated]
//! ```
//!
//! The JSON's `assets` array carries each asset's offset/length into the
//! trailing blob, so the JS side slices ArrayBuffers without copying.

mod ir;

// Conversion is allocation-bound: the XML tree allocates per element and per
// attribute, and on low-end Android the system allocator takes a lock on every
// call (measured on a Unisoc SC9832E: a third of all conversion time in
// je_malloc/je_free and their mutexes). mimalloc's thread-local free lists
// make those calls cheap. Only this library's Rust allocations use it; the
// host app's allocator is untouched.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::panic::{AssertUnwindSafe, catch_unwind};

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn fallback(reason: &str, detail: &str) -> String {
    format!(
        "{{\"status\":\"fallback\",\"reason\":\"{}\",\"detail\":\"{}\"}}",
        esc(reason),
        esc(detail)
    )
}

/// The conversion itself, safe to call from Rust. Returns the JSON and the
/// concatenated asset blob.
pub fn convert(bytes: &[u8]) -> (String, Vec<u8>) {
    let result = catch_unwind(AssertUnwindSafe(|| convert_inner(bytes)));
    match result {
        Ok(output) => output,
        // A panic in any parser must surface as the same clean fallback a
        // malformed file gets - aborting the host app is never acceptable.
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            (fallback("panic", &detail), Vec::new())
        }
    }
}

fn convert_inner(bytes: &[u8]) -> (String, Vec<u8>) {
    use anydoc::Format;

    if bytes.is_empty() {
        return (fallback("empty-input", "No document bytes were provided"), Vec::new());
    }

    let format = match Format::from_bytes(bytes) {
        Some(f) => f,
        None => {
            return (
                fallback("unsupported-format", "Could not detect a supported document format"),
                Vec::new(),
            );
        }
    };

    // PDF renders through the platform's own viewer; CSV never reaches this
    // pipeline (no content signature). Neither belongs here.
    if format == Format::Pdf {
        return (
            fallback("unsupported-format", "PDF is rendered by the platform viewer"),
            Vec::new(),
        );
    }

    let source_type = format_name(format);
    match anydoc::to_document(bytes, Some(format)) {
        Ok(doc) => {
            let output = ir::document_to_ir(&doc, source_type);
            let mut blob = Vec::new();
            for asset in &output.assets {
                blob.extend_from_slice(asset.bytes);
            }
            (output.json, blob)
        }
        Err(e) => {
            let reason = match &e {
                anydoc::ConvertError::Encrypted => "encrypted",
                _ => "parse-error",
            };
            (fallback(reason, &e.to_string()), Vec::new())
        }
    }
}

fn format_name(format: anydoc::Format) -> &'static str {
    use anydoc::Format::*;
    match format {
        Doc => "doc",
        Docx => "docx",
        Odt => "odt",
        Pdf => "pdf",
        Ppt => "ppt",
        Pptx => "pptx",
        Rtf => "rtf",
        Epub => "epub",
        Excel => "xlsx",
        Ods => "ods",
        Odp => "odp",
        Csv => "csv",
    }
}

// ------------------------------------------------------------------ C ABI ----

/// Convert `input[..input_len]`. On success writes a malloc'd buffer pointer
/// to `out` and its length to `out_len`; the caller MUST release it with
/// [`anydoc_tula_free`]. Returns 0 on success, 1 on invalid arguments. The
/// buffer layout is documented at the top of this file.
///
/// # Safety
/// `input` must point to `input_len` readable bytes; `out`/`out_len` must be
/// valid writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anydoc_tula_convert(
    input: *const u8,
    input_len: usize,
    out: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_len.is_null() {
        return 1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
    let (json, blob) = convert(bytes);

    let json_bytes = json.as_bytes();
    let total = 4 + json_bytes.len() + blob.len();
    let mut buffer = Vec::with_capacity(total);
    buffer.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    buffer.extend_from_slice(json_bytes);
    buffer.extend_from_slice(&blob);

    let mut boxed = buffer.into_boxed_slice();
    unsafe {
        *out = boxed.as_mut_ptr();
        *out_len = boxed.len();
    }
    std::mem::forget(boxed);
    0
}

/// Release a buffer produced by [`anydoc_tula_convert`].
///
/// # Safety
/// `ptr`/`len` must be exactly what `anydoc_tula_convert` produced, once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anydoc_tula_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_a_clean_fallback() {
        let (json, blob) = convert(b"this is not any kind of document");
        assert!(json.contains("\"status\":\"fallback\""));
        assert!(blob.is_empty());
    }

    #[test]
    fn empty_input_is_a_clean_fallback() {
        let (json, _) = convert(b"");
        assert!(json.contains("empty-input"));
    }

    #[test]
    fn truncated_zip_is_a_clean_fallback() {
        // A ZIP local-file-header signature and nothing else: format
        // detection may accept it, the parser must fail cleanly.
        let (json, _) = convert(b"PK\x03\x04justkidding");
        assert!(json.contains("\"status\":\"fallback\""), "got: {json}");
    }

    #[test]
    fn math_emits_flagged_runs_display_as_centered_paragraph() {
        use anydoc::model::{Block, Document, Inline};
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![
                    Inline::plain("where "),
                    Inline::Math("\\beta_{0}".to_string()),
                ]),
                Block::Math("n=\\frac{N}{1+Ne^{2}}".to_string()),
            ],
            ..Default::default()
        };
        let out = crate::ir::document_to_ir(&doc, "docx");
        assert!(
            out.json.contains("{\"text\":\"\\\\beta_{0}\",\"italic\":true,\"math\":true}"),
            "inline: {}",
            out.json
        );
        assert!(
            out.json.contains(
                "{\"type\":\"paragraph\",\"align\":\"center\",\"runs\":[{\"text\":\"n=\\\\frac{N}{1+Ne^{2}}\",\"italic\":true,\"math\":true}]}"
            ),
            "display: {}",
            out.json
        );
    }

    #[test]
    fn slide_pages_carry_their_fixed_layout() {
        use anydoc::model::{
            Block, Document, Fill, Frame, Geometry, Inline, Rgba, ShapeContent, SlideAlign,
            SlideLayout, SlideParagraph, SlideRun, SlideShape, TextAnchor, TextFrame,
        };
        let anchor = |n: u32| Block::Paragraph(vec![Inline::Anchor(format!("slide-{n}"))]);
        let px = 9525.0; // EMU per px
        let shape = SlideShape {
            frame: Frame {
                x: 10.0 * px,
                y: 20.0 * px,
                width: 300.0 * px,
                height: 40.0 * px,
                rotation: 0.0,
                flip_h: false,
                flip_v: false,
            },
            geometry: Geometry::RoundRect { radius: 0.25 },
            preset: "roundRect".to_string(),
            fill: Some(Fill::Solid(Rgba { rgb: [0x7C, 0x3A, 0xED], alpha: 0.5 })),
            line: None,
            shadow: None,
            content: ShapeContent::Text(TextFrame {
                insets: [0.0; 4],
                anchor: TextAnchor::Middle,
                wrap: true,
                paragraphs: vec![SlideParagraph {
                    align: SlideAlign::Center,
                    margin_left: 0.0,
                    indent: 0.0,
                    line_spacing: None,
                    space_before: None,
                    space_after: None,
                    bullet: None,
                    runs: vec![SlideRun {
                        text: "Hi".to_string(),
                        size: 1800.0,
                        font: Some("Calibri".to_string()),
                        bold: true,
                        italic: false,
                        underline: false,
                        strike: false,
                        color: Rgba { rgb: [255, 255, 255], alpha: 1.0 },
                        letter_spacing: 0.0,
                        baseline: 0.0,
                        all_caps: false,
                    }],
                    end_size: 1800.0,
                }],
            }),
        };
        let layout = |n: u32, shapes: Vec<SlideShape>| SlideLayout {
            anchor: format!("slide-{n}"),
            width: 1280.0 * px,
            height: 720.0 * px,
            background: None,
            shapes,
        };
        // Slide 2 has no text at all - with a layout it still gets its page.
        let doc = Document {
            blocks: vec![anchor(1), Block::Paragraph(vec![Inline::plain("Hi")]), anchor(2)],
            slides: vec![layout(1, vec![shape]), layout(2, Vec::new())],
            ..Default::default()
        };
        let out = crate::ir::document_to_ir(&doc, "pptx");
        assert!(out.json.contains("{\"pageIndex\":1,\"blocks\":[],\"slide\":{\"width\":1280,\"height\":720,\"shapes\":[]}}"), "{}", out.json);
        assert!(
            out.json.contains(
                "\"slide\":{\"width\":1280,\"height\":720,\"shapes\":[{\"x\":10,\"y\":20,\"width\":300,\"height\":40,\
                 \"geometry\":{\"type\":\"roundRect\",\"radius\":0.25},\
                 \"fill\":{\"type\":\"solid\",\"color\":\"#7C3AED80\"},\
                 \"text\":{\"insets\":[0,0,0,0],\"anchor\":\"middle\",\"paragraphs\":[{\"endSize\":24,\"align\":\"center\",\
                 \"runs\":[{\"text\":\"Hi\",\"fontSize\":24,\"color\":\"#FFFFFF\",\"fontFamily\":\"Calibri\",\"bold\":true}]}]}}]}"
            ),
            "{}",
            out.json
        );
    }

    #[test]
    fn chart_blocks_emit_typed_chart_json() {
        use anydoc::model::{Block, Chart, ChartKind, ChartSeries, Document};
        let doc = Document {
            blocks: vec![Block::Chart(Chart {
                kind: ChartKind::Pie,
                title: Some("Share".to_string()),
                axis_title: String::new(),
                categories: vec!["A".to_string(), "B".to_string()],
                series: vec![ChartSeries {
                    name: "S1".to_string(),
                    labels: vec!["1".to_string(), "2.5".to_string()],
                    values: vec![Some(1.0), Some(2.5)],
                }],
            })],
            ..Default::default()
        };
        let out = crate::ir::document_to_ir(&doc, "xlsx");
        assert!(
            out.json.contains(
                "{\"type\":\"chart\",\"kind\":\"pie\",\"title\":\"Share\",\
                 \"categories\":[\"A\",\"B\"],\
                 \"series\":[{\"name\":\"S1\",\"values\":[1,2.5],\"labels\":[\"1\",\"2.5\"]}]}"
            ),
            "got: {}",
            out.json
        );
    }
}
