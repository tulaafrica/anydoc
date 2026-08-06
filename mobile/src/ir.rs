//! anydoc document model -> Tula DocumentIR v2 JSON.
//!
//! The IR shape mirrors rn-docx-ir exactly (same field names, same units:
//! px), so the existing renderer consumes either converter's output without
//! knowing which produced it. Emitted as one JSON string by a hand-rolled
//! writer - serde would cost ~300kB of binary for a fixed, known shape.

use anydoc::model::{Align, Block, CellSlot, CommentMarkKind, Document, Inline, ParaProps, Style};

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
#[derive(Clone, PartialEq, Default)]
struct RunFormat {
    /// Ids of the comments whose range covers this run. Part of run identity:
    /// runs under different coverage must never merge, or a comment highlight
    /// bleeds past its span.
    comment_ids: Vec<String>,
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
        comment_ids: Vec::new(),
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
        if !f.comment_ids.is_empty() {
            out.push_str(",\"commentIds\":[");
            for (j, id) in f.comment_ids.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push('"');
                esc(id, out);
                out.push('"');
            }
            out.push(']');
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
    /// Comment ranges open at the current point of the walk, in opening
    /// order. Document-wide: a range may open in one paragraph and close in
    /// another, or in a table cell.
    open_comments: Vec<String>,
    /// Comments that had a range anywhere - a bare reference for one of
    /// these adds nothing (its runs are already stamped).
    had_range: std::collections::HashSet<String>,
    /// Point references seen before any text run existed to attach to.
    pending_comment_ids: Vec<String>,
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
                        let mut format = run_format(style, &emitter.doc.fonts);
                        format.comment_ids = emitter.open_comments.clone();
                        format.comment_ids.append(&mut emitter.pending_comment_ids);
                        push_run(current, text, format);
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
                        // A break inside an open comment range keeps its
                        // coverage, so the highlight is continuous.
                        let mut format = RunFormat::default();
                        format.comment_ids = emitter.open_comments.clone();
                        push_run(current, "\n", format);
                    }
                    Inline::CommentMark { id, kind } => match kind {
                        CommentMarkKind::RangeStart => {
                            emitter.open_comments.push(id.clone());
                            emitter.had_range.insert(id.clone());
                        }
                        CommentMarkKind::RangeEnd => {
                            if let Some(pos) =
                                emitter.open_comments.iter().position(|open| open == id)
                            {
                                emitter.open_comments.remove(pos);
                            }
                        }
                        CommentMarkKind::Reference => {
                            // The mark sits at the END of the span. Ranged
                            // comments are already stamped; a point comment
                            // anchors on the nearest run.
                            if !emitter.had_range.contains(id) {
                                match current.last_mut() {
                                    Some(last) => last.format.comment_ids.push(id.clone()),
                                    None => emitter.pending_comment_ids.push(id.clone()),
                                }
                            }
                        }
                    },
                    // Zero-width in the source; nothing to draw. Slide anchors
                    // are handled a level up as page breaks; ParaPres is
                    // lifted onto the block by write_block.
                    Inline::Anchor(_) | Inline::NoteRef(_) | Inline::ParaPres(_) => {}
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
                let props = para_props_of(content).copied();
                let segments = self.segments(content);
                for segment in segments {
                    self.write_segment_block(
                        segment,
                        Some((*level).clamp(1, 6)),
                        props,
                        out,
                        first,
                    );
                }
            }
            Block::Paragraph(inlines) => {
                let only_marks = !inlines.is_empty()
                    && inlines.iter().all(|i| matches!(i, Inline::CommentMark { .. }));
                let props = para_props_of(inlines).copied();
                let segments = self.segments(inlines);
                if only_marks {
                    // Synthetic carrier for block-level comment markers: the
                    // segments() call above updated the tracker; nothing to draw.
                    return;
                }
                if segments.is_empty() {
                    // A genuinely empty paragraph is meaningful vertical space
                    // - and its spacing (a spacer paragraph) still matters.
                    sep(out, first);
                    out.push_str("{\"type\":\"paragraph\",\"runs\":[]");
                    if let Some(p) = &props {
                        write_layout(p, out);
                    }
                    out.push('}');
                    return;
                }
                for segment in segments {
                    self.write_segment_block(segment, None, props, out, first);
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
                        if let Some(t) = cell.width_twips {
                            out.push_str(&format!(",\"width\":{}", twips_to_px(t)));
                        }
                        if let Some([r, g, b]) = cell.background {
                            out.push(',');
                            str_field(out, "background", &format!("#{r:02X}{g:02X}{b:02X}"));
                        }
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
                if let Some(borders) = &table.borders {
                    // rn-docx-ir's shape: px width (eighths of a point ->
                    // px, min 1 so a hairline still draws), colour with
                    // #000000 standing in for auto.
                    let edge_px = |e: &anydoc::model::BorderEdge| {
                        let px = ((e.width_eighths as f64) / 8.0 * 96.0 / 72.0).round().max(1.0);
                        let color = e
                            .color
                            .map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"))
                            .unwrap_or_else(|| "#000000".to_string());
                        format!("{{\"width\":{px},\"color\":\"{color}\"}}")
                    };
                    let mut group = String::new();
                    for (key, e) in [
                        ("top", borders.top),
                        ("bottom", borders.bottom),
                        ("left", borders.left),
                        ("right", borders.right),
                        ("insideH", borders.inside_h),
                        ("insideV", borders.inside_v),
                    ] {
                        if let Some(e) = e {
                            if !group.is_empty() {
                                group.push(',');
                            }
                            group.push_str(&format!("\"{key}\":{}", edge_px(&e)));
                        }
                    }
                    out.push_str(&format!(",\"borders\":{{{group}}}"));
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
        props: Option<ParaProps>,
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
                if let Some(p) = &props {
                    write_layout(p, out);
                }
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

/// Paragraph layout as IR JSON fields, appended INSIDE an open block object.
/// Mirrors rn-docx-ir's rules exactly: left alignment is the default and is
/// omitted; twips convert to px; zeros are omitted; empty groups are omitted.
fn write_layout(props: &ParaProps, out: &mut String) {
    match props.align {
        Some(Align::Center) => out.push_str(",\"align\":\"center\""),
        Some(Align::Right) => out.push_str(",\"align\":\"right\""),
        Some(Align::Justify) => out.push_str(",\"align\":\"justify\""),
        Some(Align::Left) | None => {}
    }

    let px = |t: i32| -> i64 { ((t as f64) / 15.0).round() as i64 };
    let mut group = String::new();
    for (key, value) in [
        ("start", props.indent_start),
        ("end", props.indent_end),
        ("firstLine", props.indent_first_line),
        ("hanging", props.indent_hanging),
    ] {
        if let Some(t) = value
            && px(t) != 0
        {
            if !group.is_empty() {
                group.push(',');
            }
            group.push_str(&format!("\"{key}\":{}", px(t)));
        }
    }
    if !group.is_empty() {
        out.push_str(&format!(",\"indent\":{{{group}}}"));
    }

    group = String::new();
    for (key, value) in [("before", props.spacing_before), ("after", props.spacing_after)] {
        if let Some(t) = value
            && px(t as i32) != 0
        {
            if !group.is_empty() {
                group.push(',');
            }
            group.push_str(&format!("\"{key}\":{}", px(t as i32)));
        }
    }
    if let Some(line) = props.line_240ths
        && line > 0
        && line != 240
    {
        if !group.is_empty() {
            group.push(',');
        }
        group.push_str(&format!("\"lineHeightMultiple\":{}", (line as f64) / 240.0));
    }
    if !group.is_empty() {
        out.push_str(&format!(",\"spacing\":{{{group}}}"));
    }
}

/// A comment body flattened to text: paragraphs joined by newlines. Comment
/// prose is not worth carrying formatting for - same rule as rn-docx-ir.
fn comment_text(blocks: &[Block]) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    fn collect(blocks: &[Block], out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                    let text = anydoc::model::inlines_to_plain_text(inlines);
                    if !text.trim().is_empty() {
                        out.push(text);
                    }
                }
                Block::List(list) => {
                    for item in &list.items {
                        collect(&item.blocks, out);
                    }
                }
                Block::BlockQuote(inner) => collect(inner, out),
                _ => {}
            }
        }
    }
    collect(blocks, &mut paragraphs);
    paragraphs.join("\n")
}

/// The leading ParaPres marker of a paragraph's inline stream, if any.
fn para_props_of(inlines: &[Inline]) -> Option<&ParaProps> {
    inlines.iter().find_map(|i| match i {
        Inline::ParaPres(p) => Some(p.as_ref()),
        _ => None,
    })
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

    let mut emitter = Emitter {
        doc,
        used: Vec::new(),
        open_comments: Vec::new(),
        had_range: std::collections::HashSet::new(),
        pending_comment_ids: Vec::new(),
    };
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
    json.push_str("]");
    if !doc.comments.is_empty() {
        json.push_str(",\"comments\":[");
        for (i, comment) in doc.comments.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push('{');
            str_field(&mut json, "id", &comment.id);
            if let Some(author) = &comment.author {
                json.push(',');
                str_field(&mut json, "author", author);
            }
            if let Some(initials) = &comment.initials {
                json.push(',');
                str_field(&mut json, "initials", initials);
            }
            if let Some(date) = &comment.date {
                json.push(',');
                str_field(&mut json, "date", date);
            }
            json.push(',');
            str_field(&mut json, "text", &comment_text(&comment.blocks));
            json.push('}');
        }
        json.push(']');
    }
    json.push_str("},\"assets\":[");

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
