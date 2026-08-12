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
