//! anydoc document model -> Tula DocumentIR v2 JSON.
//!
//! The IR shape mirrors rn-docx-ir exactly (same field names, same units:
//! px), so the existing renderer consumes either converter's output without
//! knowing which produced it. Emitted as one JSON string by a hand-rolled
//! writer - serde would cost ~300kB of binary for a fixed, known shape.

use anydoc::model::{Block, CellSlot, Document, Inline, Style};

/// A resolved asset the caller must carry alongside the IR.
pub struct IrAsset<'a> {
    pub asset_ref: String,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

pub struct IrOutput<'a> {
    pub json: String,
    pub assets: Vec<IrAsset<'a>>,
}

/// Half-points (w:sz) to CSS/RN px: pt = hp/2, px = pt * 96/72.
fn half_points_to_px(hp: u16) -> u32 {
    ((hp as f64) * 2.0 / 3.0).round() as u32
}

/// Twips to px, same rule as rn-docx-ir (twips / 15).
fn twips_to_px(twips: u32) -> u32 {
    ((twips as f64) / 15.0).round() as u32
}

fn extension_for(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        "image/webp" => "webp",
        "image/x-emf" => "emf",
        "image/x-wmf" => "wmf",
        _ => "bin",
    }
}

// ------------------------------------------------------------- JSON bits ----

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

fn str_field(out: &mut String, key: &str, value: &str) {
    out.push('"');
    out.push_str(key);
    out.push_str("\":\"");
    esc(value, out);
    out.push('"');
}

// ------------------------------------------------------------------ runs ----

/// One IR text run. Formatting fields mirror rn-docx-ir's TextRun; only set
/// fields are emitted (the IR is encrypted into a message body - an unset
/// property must not cost bytes).
#[derive(Clone, PartialEq)]
struct RunFormat {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    highlight: Option<&'static str>,
    font: Option<String>,
    size_px: Option<u32>,
    color: Option<String>,
    vert: Option<&'static str>,
    caps: Option<&'static str>,
}

struct Run {
    text: String,
    format: RunFormat,
}

fn run_format(style: &Style, fonts: &[String]) -> RunFormat {
    let font = style
        .font
        .and_then(|id| fonts.get(id.0 as usize))
        .cloned()
        // `code` with no explicit face still needs a monospace hint.
        .or_else(|| if style.code { Some("Courier New".to_string()) } else { None });
    RunFormat {
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strikethrough: style.strike,
        highlight: style.highlight.map(|h| h.name()),
        font,
        size_px: style.size_half_points.map(half_points_to_px),
        color: style.color.map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}")),
        vert: style.vert_align.map(|v| match v {
            anydoc::model::VertAlign::Superscript => "super",
            anydoc::model::VertAlign::Subscript => "sub",
        }),
        caps: style.caps.map(|c| match c {
            anydoc::model::Caps::All => "all",
            anydoc::model::Caps::Small => "small",
        }),
    }
}

fn write_runs(runs: &[Run], out: &mut String) {
    out.push('[');
    for (i, run) in runs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        str_field(out, "text", &run.text);
        let f = &run.format;
        if f.bold {
            out.push_str(",\"bold\":true");
        }
        if f.italic {
            out.push_str(",\"italic\":true");
        }
        if f.underline {
            out.push_str(",\"underline\":true");
        }
        if f.strikethrough {
            out.push_str(",\"strikethrough\":true");
        }
        if let Some(h) = f.highlight {
            out.push(',');
            str_field(out, "highlightColor", h);
        }
        if let Some(font) = &f.font {
            out.push(',');
            str_field(out, "fontFamily", font);
        }
        if let Some(px) = f.size_px {
            out.push_str(&format!(",\"fontSize\":{px}"));
        }
        if let Some(color) = &f.color {
            out.push(',');
            str_field(out, "color", color);
        }
        if let Some(v) = f.vert {
            out.push(',');
            str_field(out, "verticalAlign", v);
        }
        if let Some(c) = f.caps {
            out.push(',');
            str_field(out, "caps", c);
        }
        out.push('}');
    }
    out.push(']');
}

// -------------------------------------------------------------- traversal ----

struct Emitter<'a> {
    doc: &'a Document,
    /// Assets actually referenced by an emitted image block, in first-use
    /// order. (asset index in doc.assets, assetRef)
    used: Vec<(usize, String)>,
}

enum Segment {
    Runs(Vec<Run>),
    Image(String),
}

impl<'a> Emitter<'a> {
    fn asset_ref(&mut self, id: usize) -> Option<String> {
        if let Some((_, r)) = self.used.iter().find(|(i, _)| *i == id) {
            return Some(r.clone());
        }
        let asset = self.doc.assets.get(id)?;
        let r = format!("image{id}.{}", extension_for(&asset.media_type));
        self.used.push((id, r.clone()));
        Some(r)
    }

    /// Inline stream -> runs and image breaks, in document order. Adjacent
    /// same-format runs merge, mirroring rn-docx-ir's mergeRuns.
    fn segments(&mut self, inlines: &[Inline]) -> Vec<Segment> {
        let mut segments: Vec<Segment> = Vec::new();
        let mut current: Vec<Run> = Vec::new();

        fn push_run(current: &mut Vec<Run>, text: &str, format: RunFormat) {
            if text.is_empty() {
                return;
            }
            if let Some(last) = current.last_mut()
                && last.format == format
            {
                last.text.push_str(text);
                return;
            }
            current.push(Run { text: text.to_string(), format });
        }

        fn walk<'i>(
            emitter: &mut Emitter,
            inlines: &'i [Inline],
            current: &mut Vec<Run>,
            segments: &mut Vec<Segment>,
        ) {
            for inline in inlines {
                match inline {
                    Inline::Text { text, style } => {
                        push_run(current, text, run_format(style, &emitter.doc.fonts));
                    }
                    Inline::Link { content, .. } => {
                        // IR v2 has no link type: keep the text, lose the href.
                        walk(emitter, content, current, segments);
                    }
                    Inline::Image { source, .. } => {
                        if let anydoc::model::ImageSource::Asset(id) = source
                            && let Some(r) = emitter.asset_ref(id.0)
                        {
                            if !current.is_empty() {
                                segments.push(Segment::Runs(std::mem::take(current)));
                            }
                            segments.push(Segment::Image(r));
                        }
                        // External/unavailable images are dropped, same call
                        // as rn-docx-ir: fetching would leak the open.
                    }
                    Inline::LineBreak => {
                        push_run(
                            current,
                            "\n",
                            RunFormat {
                                bold: false,
                                italic: false,
                                underline: false,
                                strikethrough: false,
                                highlight: None,
                                font: None,
                                size_px: None,
                                color: None,
                                vert: None,
                                caps: None,
                            },
                        );
                    }
                    // Zero-width in the source; nothing to draw. Slide anchors
                    // are handled a level up as page breaks.
                    Inline::Anchor(_) | Inline::NoteRef(_) => {}
                }
            }
        }

        walk(self, inlines, &mut current, &mut segments);
        if !current.is_empty() {
            segments.push(Segment::Runs(current));
        }
        segments
    }

    fn runs_only(&mut self, inlines: &[Inline]) -> Vec<Run> {
        let mut runs = Vec::new();
        for segment in self.segments(inlines) {
            if let Segment::Runs(r) = segment {
                runs.extend(r);
            }
            // An image inside a table cell has nowhere to go in the IR;
            // rn-docx-ir drops it with a warning, we do the same silently
            // at this layer (the caller counts warnings).
        }
        runs
    }

    fn write_block(&mut self, block: &Block, out: &mut String, first: &mut bool) {
        match block {
            Block::Heading { level, content, .. } => {
                let segments = self.segments(content);
                for segment in segments {
                    self.write_segment_block(segment, Some((*level).clamp(1, 6)), out, first);
                }
            }
            Block::Paragraph(inlines) => {
                let segments = self.segments(inlines);
                if segments.is_empty() {
                    // A genuinely empty paragraph is meaningful vertical space.
                    sep(out, first);
                    out.push_str("{\"type\":\"paragraph\",\"runs\":[]}");
                    return;
                }
                for segment in segments {
                    self.write_segment_block(segment, None, out, first);
                }
            }
            Block::List(list) => {
                sep(out, first);
                let ordered = list.marker != anydoc::model::MarkerKind::Bullet;
                out.push_str(&format!("{{\"type\":\"list\",\"ordered\":{ordered},\"items\":["));
                let mut first_item = true;
                self.write_list_items(list, 0, out, &mut first_item);
                out.push_str("]}");
            }
            Block::Table(table) => {
                sep(out, first);
                out.push_str("{\"type\":\"table\",\"rows\":[");
                for (ri, row) in table.grid.iter().enumerate() {
                    if ri > 0 {
                        out.push(',');
                    }
                    out.push('[');
                    let mut first_cell = true;
                    for slot in row {
                        let CellSlot::Origin(cell) = slot else { continue };
                        if !first_cell {
                            out.push(',');
                        }
                        first_cell = false;
                        out.push_str("{\"paragraphs\":[");
                        let mut first_para = true;
                        for b in &cell.blocks {
                            let inlines: &[Inline] = match b {
                                Block::Paragraph(i) => i,
                                Block::Heading { content, .. } => content,
                                _ => continue,
                            };
                            let runs = self.runs_only(inlines);
                            if runs.is_empty() {
                                continue;
                            }
                            if !first_para {
                                out.push(',');
                            }
                            first_para = false;
                            write_runs(&runs, out);
                        }
                        out.push(']');
                        if cell.col_span > 1 {
                            out.push_str(&format!(",\"colSpan\":{}", cell.col_span));
                        }
                        if cell.row_span > 1 {
                            out.push_str(&format!(",\"rowSpan\":{}", cell.row_span));
                        }
                        out.push('}');
                    }
                    out.push(']');
                }
                out.push(']');
                if let Some(widths) = &table.column_widths {
                    out.push_str(",\"columnWidths\":[");
                    for (i, w) in widths.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&twips_to_px(*w).to_string());
                    }
                    out.push(']');
                }
                out.push('}');
            }
            Block::BlockQuote(inner) => {
                // No quote block in the IR; keep the text.
                for b in inner {
                    self.write_block(b, out, first);
                }
            }
            Block::CodeBlock { text, .. } => {
                if text.is_empty() {
                    return;
                }
                sep(out, first);
                out.push_str("{\"type\":\"paragraph\",\"runs\":[{");
                str_field(out, "text", text);
                out.push(',');
                str_field(out, "fontFamily", "Courier New");
                out.push_str("}]}");
            }
            Block::Rule => {
                sep(out, first);
                out.push_str("{\"type\":\"paragraph\",\"runs\":[]}");
            }
        }
    }

    fn write_segment_block(
        &mut self,
        segment: Segment,
        heading: Option<u8>,
        out: &mut String,
        first: &mut bool,
    ) {
        match segment {
            Segment::Runs(runs) => {
                sep(out, first);
                match heading {
                    Some(level) => {
                        out.push_str(&format!("{{\"type\":\"heading\",\"level\":{level},\"runs\":"))
                    }
                    None => out.push_str("{\"type\":\"paragraph\",\"runs\":"),
                }
                write_runs(&runs, out);
                out.push('}');
            }
            Segment::Image(asset_ref) => {
                sep(out, first);
                out.push_str("{\"type\":\"image\",");
                str_field(out, "assetRef", &asset_ref);
                out.push('}');
            }
        }
    }

    fn write_list_items(
        &mut self,
        list: &anydoc::model::List,
        level: usize,
        out: &mut String,
        first_item: &mut bool,
    ) {
        for item in &list.items {
            for block in &item.blocks {
                match block {
                    Block::List(nested) => {
                        self.write_list_items(nested, level + 1, out, first_item);
                    }
                    Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                        let runs = self.runs_only(inlines);
                        if runs.is_empty() {
                            continue;
                        }
                        if !*first_item {
                            out.push(',');
                        }
                        *first_item = false;
                        out.push_str("{\"runs\":");
                        write_runs(&runs, out);
                        out.push_str(&format!(",\"level\":{level}}}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

fn sep(out: &mut String, first: &mut bool) {
    if !*first {
        out.push(',');
    }
    *first = false;
}

/// Is this paragraph a pptx slide boundary - a single `slide-N` anchor?
fn slide_anchor(block: &Block) -> bool {
    let Block::Paragraph(inlines) = block else { return false };
    let [Inline::Anchor(id)] = inlines.as_slice() else { return false };
    id.strip_prefix("slide-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The whole document as IR v2 JSON plus the referenced asset bytes.
///
/// pptx slide anchors become PAGE boundaries: the IR's page list is the one
/// place the renderer already knows how to paginate.
pub fn document_to_ir<'a>(doc: &'a Document, source_type: &str) -> IrOutput<'a> {
    // Split blocks into pages at slide anchors.
    let mut pages: Vec<Vec<&Block>> = vec![Vec::new()];
    for block in &doc.blocks {
        if slide_anchor(block) {
            if !pages.last().unwrap().is_empty() {
                pages.push(Vec::new());
            }
            continue;
        }
        pages.last_mut().unwrap().push(block);
    }

    let mut emitter = Emitter { doc, used: Vec::new() };
    let mut json = String::with_capacity(16 * 1024);
    json.push_str("{\"status\":\"ok\",\"ir\":{\"version\":2,");
    str_field(&mut json, "sourceType", source_type);
    json.push_str(",\"pages\":[");
    for (pi, page) in pages.iter().enumerate() {
        if pi > 0 {
            json.push(',');
        }
        json.push_str(&format!("{{\"pageIndex\":{pi},\"blocks\":["));
        let mut first = true;
        for block in page {
            emitter.write_block(block, &mut json, &mut first);
        }
        json.push_str("]}");
    }
    json.push_str("]},\"assets\":[");

    let mut assets = Vec::new();
    let mut offset = 0usize;
    for (i, (id, asset_ref)) in emitter.used.iter().enumerate() {
        let asset = &doc.assets[*id];
        if i > 0 {
            json.push(',');
        }
        json.push('{');
        str_field(&mut json, "assetRef", asset_ref);
        json.push(',');
        str_field(&mut json, "contentType", &asset.media_type);
        json.push_str(&format!(",\"offset\":{offset},\"length\":{}}}", asset.bytes.len()));
        offset += asset.bytes.len();
        assets.push(IrAsset {
            asset_ref: asset_ref.clone(),
            content_type: &asset.media_type,
            bytes: &asset.bytes,
        });
    }
    json.push_str("]}");

    IrOutput { json, assets }
}
