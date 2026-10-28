//! anydoc document model -> Tula DocumentIR v2 JSON.
//!
//! The IR shape mirrors rn-docx-ir exactly (same field names, same units:
//! px), so the existing renderer consumes either converter's output without
//! knowing which produced it. Emitted as one JSON string by a hand-rolled
//! writer - serde would cost ~300kB of binary for a fixed, known shape.

use anydoc::model::{
    Align, Block, CellSlot, CommentMarkKind, Document, Fill, Geometry, ImageSource, Inline, Line,
    LineDash, ParaProps, PathCmd, Rgba, ShapeContent, SlideAlign, SlideLayout, SlideParagraph,
    Spacing, Style, TextAnchor,
};

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
    /// The run's text is LaTeX source (an equation), not prose. Renderers
    /// that can typeset do; the italic flag rides along so older ones show
    /// the source legibly.
    math: bool,
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
        math: false,
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
        if f.math {
            out.push_str(",\"math\":true");
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
                    Inline::Math(tex) => {
                        // No math renderer in the IR yet: the LaTeX source,
                        // italic, keeps the formula legible and searchable
                        // rather than dropping it.
                        let mut format =
                            RunFormat { italic: true, math: true, ..RunFormat::default() };
                        format.comment_ids = emitter.open_comments.clone();
                        push_run(current, tex.trim(), format);
                    }
                    Inline::Checkbox(checked) => {
                        let mut format = RunFormat::default();
                        format.comment_ids = emitter.open_comments.clone();
                        push_run(current, if *checked { "\u{2611} " } else { "\u{2610} " }, format);
                    }
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
            Block::Math(tex) => {
                // A displayed formula: its LaTeX source as a centered italic
                // paragraph (no math renderer in the IR yet).
                let tex = tex.trim();
                if tex.is_empty() {
                    return;
                }
                // A paragraph whose only run is math IS display math to a
                // renderer that typesets; older ones show centered italic
                // source. No new block type, so nothing is ever dropped.
                sep(out, first);
                out.push_str("{\"type\":\"paragraph\",\"align\":\"center\",\"runs\":[{");
                str_field(out, "text", tex);
                out.push_str(",\"italic\":true,\"math\":true}]}");
            }
            Block::Chart(chart) => {
                // The typed chart block, followed by nothing else: a renderer
                // that can draw uses kind + numeric series; one that cannot
                // rebuilds the titled data table from categories x labels.
                sep(out, first);
                write_chart(chart, out);
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

/// A chart as its IR block: kind, title, categories, numeric series.
fn write_chart(chart: &anydoc::model::Chart, out: &mut String) {
    out.push_str("{\"type\":\"chart\",");
    str_field(out, "kind", chart.kind.as_str());
    if let Some(title) = &chart.title {
        out.push(',');
        str_field(out, "title", title);
    }
    if !chart.axis_title.is_empty() {
        out.push(',');
        str_field(out, "axisTitle", &chart.axis_title);
    }
    out.push_str(",\"categories\":[");
    for (i, cat) in chart.categories.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        esc(cat, out);
        out.push('"');
    }
    out.push_str("],\"series\":[");
    for (si, s) in chart.series.iter().enumerate() {
        if si > 0 {
            out.push(',');
        }
        out.push('{');
        str_field(out, "name", &s.name);
        out.push_str(",\"values\":[");
        for (i, v) in s.values.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            match v {
                Some(n) if n.is_finite() => out.push_str(&n.to_string()),
                _ => out.push_str("null"),
            }
        }
        out.push_str("],\"labels\":[");
        for (i, l) in s.labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            esc(l, out);
            out.push('"');
        }
        out.push_str("]}");
    }
    out.push_str("]}");
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

// ----------------------------------------------------------------- slides ----
//
// A slide page carries `slide`: the deck drawn as PowerPoint draws it, every
// shape at its own position. Lengths are px at 96 dpi (EMU / 9525) so the
// whole page scales by one factor; font sizes are px too. Numbers round to
// 0.01 px - finer than any screen, coarse enough to keep the JSON small.

fn emu_px(v: f64) -> f64 {
    v / 9525.0
}

/// Hundredths of a point to px.
fn hpt_px(v: f64) -> f64 {
    v / 75.0
}

fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.2}");
        s.trim_end_matches('0').to_string()
    }
}

/// `#RRGGBB`, or `#RRGGBBAA` when translucent.
fn color_hex(c: Rgba) -> String {
    let [r, g, b] = c.rgb;
    if c.alpha >= 0.999 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        let a = (c.alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

fn path_d(commands: &[PathCmd]) -> String {
    let p = |v: f64| n(emu_px(v));
    let mut d = String::new();
    for cmd in commands {
        if !d.is_empty() {
            d.push(' ');
        }
        match *cmd {
            PathCmd::MoveTo(x, y) => d.push_str(&format!("M{} {}", p(x), p(y))),
            PathCmd::LineTo(x, y) => d.push_str(&format!("L{} {}", p(x), p(y))),
            PathCmd::CubicTo(a, b, c, e, x, y) => {
                d.push_str(&format!("C{} {} {} {} {} {}", p(a), p(b), p(c), p(e), p(x), p(y)))
            }
            PathCmd::QuadTo(a, b, x, y) => {
                d.push_str(&format!("Q{} {} {} {}", p(a), p(b), p(x), p(y)))
            }
            PathCmd::ArcTo { rx, ry, large, sweep, x, y } => d.push_str(&format!(
                "A{} {} 0 {} {} {} {}",
                p(rx),
                p(ry),
                u8::from(large),
                u8::from(sweep),
                p(x),
                p(y)
            )),
            PathCmd::Close => d.push('Z'),
        }
    }
    d
}

fn write_line(line: &Line, out: &mut String) {
    out.push_str(&format!("{{\"width\":{},", n(emu_px(line.width))));
    str_field(out, "color", &color_hex(line.color));
    let dash = match line.dash {
        LineDash::Solid => None,
        LineDash::Dash => Some("dash"),
        LineDash::Dot => Some("dot"),
        LineDash::DashDot => Some("dashDot"),
    };
    if let Some(d) = dash {
        out.push(',');
        str_field(out, "dash", d);
    }
    if let Some(h) = &line.head_end {
        out.push(',');
        str_field(out, "headEnd", h);
    }
    if let Some(t) = &line.tail_end {
        out.push(',');
        str_field(out, "tailEnd", t);
    }
    out.push('}');
}

fn write_spacing(key: &str, s: Option<Spacing>, out: &mut String) {
    match s {
        Some(Spacing::Lines(v)) => out.push_str(&format!(",\"{key}\":{{\"lines\":{}}}", n(v))),
        Some(Spacing::Points(v)) => {
            out.push_str(&format!(",\"{key}\":{{\"px\":{}}}", n(hpt_px(v))))
        }
        None => {}
    }
}

fn write_insets(insets: &[f64; 4], out: &mut String) {
    out.push_str(&format!(
        "\"insets\":[{},{},{},{}]",
        n(emu_px(insets[0])),
        n(emu_px(insets[1])),
        n(emu_px(insets[2])),
        n(emu_px(insets[3]))
    ));
}

fn anchor_name(a: TextAnchor) -> Option<&'static str> {
    match a {
        TextAnchor::Top => None,
        TextAnchor::Middle => Some("middle"),
        TextAnchor::Bottom => Some("bottom"),
    }
}

fn write_paragraphs(paragraphs: &[SlideParagraph], out: &mut String) {
    out.push_str("\"paragraphs\":[");
    for (i, p) in paragraphs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        out.push_str(&format!("\"endSize\":{}", n(hpt_px(p.end_size))));
        let align = match p.align {
            SlideAlign::Left => None,
            SlideAlign::Center => Some("center"),
            SlideAlign::Right => Some("right"),
            SlideAlign::Justify => Some("justify"),
        };
        if let Some(a) = align {
            out.push(',');
            str_field(out, "align", a);
        }
        if p.margin_left != 0.0 {
            out.push_str(&format!(",\"marginLeft\":{}", n(emu_px(p.margin_left))));
        }
        if p.indent != 0.0 {
            out.push_str(&format!(",\"indent\":{}", n(emu_px(p.indent))));
        }
        write_spacing("lineSpacing", p.line_spacing, out);
        write_spacing("spaceBefore", p.space_before, out);
        write_spacing("spaceAfter", p.space_after, out);
        if let Some(b) = &p.bullet {
            out.push_str(",\"bullet\":{");
            str_field(out, "text", &b.text);
            if let Some(c) = b.color {
                out.push(',');
                str_field(out, "color", &color_hex(c));
            }
            if let Some(f) = &b.font {
                out.push(',');
                str_field(out, "fontFamily", f);
            }
            if (b.size - 1.0).abs() > 1e-6 {
                out.push_str(&format!(",\"size\":{}", n(b.size)));
            }
            out.push('}');
        }
        out.push_str(",\"runs\":[");
        for (j, r) in p.runs.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push('{');
            str_field(out, "text", &r.text);
            out.push_str(&format!(",\"fontSize\":{},", n(hpt_px(r.size))));
            str_field(out, "color", &color_hex(r.color));
            if let Some(f) = &r.font {
                out.push(',');
                str_field(out, "fontFamily", f);
            }
            for (flag, key) in [
                (r.bold, "bold"),
                (r.italic, "italic"),
                (r.underline, "underline"),
                (r.strike, "strikethrough"),
            ] {
                if flag {
                    out.push_str(&format!(",\"{key}\":true"));
                }
            }
            if r.letter_spacing != 0.0 {
                out.push_str(&format!(",\"letterSpacing\":{}", n(hpt_px(r.letter_spacing))));
            }
            if r.baseline != 0.0 {
                out.push_str(&format!(",\"baseline\":{}", n(r.baseline)));
            }
            if r.all_caps {
                out.push_str(",\"caps\":\"all\"");
            }
            out.push('}');
        }
        out.push_str("]}");
    }
    out.push(']');
}

impl Emitter<'_> {
    /// A fill object, or `None` when it cannot be drawn (an image the IR
    /// does not carry).
    fn fill_json(&mut self, fill: &Fill) -> Option<String> {
        let mut out = String::new();
        match fill {
            Fill::Solid(c) => {
                out.push_str("{\"type\":\"solid\",");
                str_field(&mut out, "color", &color_hex(*c));
                out.push('}');
            }
            Fill::Gradient { angle, radial, stops } => {
                out.push_str(&format!("{{\"type\":\"gradient\",\"angle\":{}", n(*angle)));
                if *radial {
                    out.push_str(",\"radial\":true");
                }
                out.push_str(",\"stops\":[");
                for (i, (pos, c)) in stops.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!("{{\"offset\":{},", n(*pos)));
                    str_field(&mut out, "color", &color_hex(*c));
                    out.push('}');
                }
                out.push_str("]}");
            }
            Fill::Image(ImageSource::Asset(id)) => {
                let r = self.asset_ref(id.0)?;
                out.push_str("{\"type\":\"image\",");
                str_field(&mut out, "assetRef", &r);
                out.push('}');
            }
            Fill::Image(_) => return None,
        }
        Some(out)
    }

    fn write_slide(&mut self, slide: &SlideLayout, out: &mut String) {
        out.push_str(&format!(
            "{{\"width\":{},\"height\":{}",
            n(emu_px(slide.width)),
            n(emu_px(slide.height))
        ));
        if let Some(json) = slide.background.as_ref().and_then(|f| self.fill_json(f)) {
            out.push_str(",\"background\":");
            out.push_str(&json);
        }
        out.push_str(",\"shapes\":[");
        for (i, shape) in slide.shapes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let f = &shape.frame;
            out.push_str(&format!(
                "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}",
                n(emu_px(f.x)),
                n(emu_px(f.y)),
                n(emu_px(f.width)),
                n(emu_px(f.height))
            ));
            if f.rotation != 0.0 {
                out.push_str(&format!(",\"rotation\":{}", n(f.rotation)));
            }
            if f.flip_h {
                out.push_str(",\"flipH\":true");
            }
            if f.flip_v {
                out.push_str(",\"flipV\":true");
            }
            out.push_str(",\"geometry\":");
            match &shape.geometry {
                Geometry::Rect => out.push_str("{\"type\":\"rect\"}"),
                Geometry::RoundRect { radius } => {
                    out.push_str(&format!("{{\"type\":\"roundRect\",\"radius\":{}}}", n(*radius)))
                }
                Geometry::Ellipse => out.push_str("{\"type\":\"ellipse\"}"),
                Geometry::Line => out.push_str("{\"type\":\"line\"}"),
                Geometry::Path { commands, filled } => {
                    out.push_str("{\"type\":\"path\",");
                    str_field(out, "d", &path_d(commands));
                    if !filled {
                        out.push_str(",\"filled\":false");
                    }
                    out.push('}');
                }
            }
            if !matches!(shape.preset.as_str(), "rect" | "roundRect" | "ellipse" | "line") {
                out.push(',');
                str_field(out, "preset", &shape.preset);
            }
            if let Some(json) = shape.fill.as_ref().and_then(|fill| self.fill_json(fill)) {
                out.push_str(",\"fill\":");
                out.push_str(&json);
            }
            if let Some(line) = &shape.line {
                out.push_str(",\"line\":");
                write_line(line, out);
            }
            if let Some(sh) = &shape.shadow {
                out.push_str(&format!(
                    ",\"shadow\":{{\"dx\":{},\"dy\":{},\"blur\":{},",
                    n(emu_px(sh.dx)),
                    n(emu_px(sh.dy)),
                    n(emu_px(sh.blur))
                ));
                str_field(out, "color", &color_hex(sh.color));
                out.push('}');
            }
            match &shape.content {
                ShapeContent::None => {}
                ShapeContent::Text(t) => {
                    out.push_str(",\"text\":{");
                    write_insets(&t.insets, out);
                    if let Some(a) = anchor_name(t.anchor) {
                        out.push(',');
                        str_field(out, "anchor", a);
                    }
                    if !t.wrap {
                        out.push_str(",\"wrap\":false");
                    }
                    out.push(',');
                    write_paragraphs(&t.paragraphs, out);
                    out.push('}');
                }
                ShapeContent::Image { source, crop } => {
                    out.push_str(",\"image\":{");
                    let mut wrote = false;
                    if let ImageSource::Asset(id) = source
                        && let Some(r) = self.asset_ref(id.0)
                    {
                        str_field(out, "assetRef", &r);
                        wrote = true;
                    }
                    if crop.left != 0.0
                        || crop.top != 0.0
                        || crop.right != 0.0
                        || crop.bottom != 0.0
                    {
                        if wrote {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "\"crop\":{{\"left\":{},\"top\":{},\"right\":{},\"bottom\":{}}}",
                            n(crop.left),
                            n(crop.top),
                            n(crop.right),
                            n(crop.bottom)
                        ));
                    }
                    out.push('}');
                }
                ShapeContent::Table(table) => {
                    out.push_str(",\"table\":{\"columns\":[");
                    for (i, c) in table.columns.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&n(emu_px(*c)));
                    }
                    out.push_str("],\"rows\":[");
                    for (ri, row) in table.rows.iter().enumerate() {
                        if ri > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"height\":{},\"cells\":[",
                            n(emu_px(row.height))
                        ));
                        for (ci, cell) in row.cells.iter().enumerate() {
                            if ci > 0 {
                                out.push(',');
                            }
                            out.push_str(&format!("{{\"col\":{}", cell.col));
                            if cell.col_span > 1 {
                                out.push_str(&format!(",\"colSpan\":{}", cell.col_span));
                            }
                            if cell.row_span > 1 {
                                out.push_str(&format!(",\"rowSpan\":{}", cell.row_span));
                            }
                            if let Some(json) = cell.fill.as_ref().and_then(|f| self.fill_json(f)) {
                                out.push_str(",\"fill\":");
                                out.push_str(&json);
                            }
                            if cell.borders.iter().any(Option::is_some) {
                                out.push_str(",\"borders\":{");
                                let mut first_edge = true;
                                for (key, edge) in
                                    ["left", "top", "right", "bottom"].iter().zip(&cell.borders)
                                {
                                    if let Some(line) = edge {
                                        if !first_edge {
                                            out.push(',');
                                        }
                                        first_edge = false;
                                        out.push_str(&format!("\"{key}\":"));
                                        write_line(line, out);
                                    }
                                }
                                out.push('}');
                            }
                            out.push(',');
                            write_insets(&cell.insets, out);
                            if let Some(a) = anchor_name(cell.anchor) {
                                out.push(',');
                                str_field(out, "anchor", a);
                            }
                            out.push(',');
                            write_paragraphs(&cell.paragraphs, out);
                            out.push('}');
                        }
                        out.push_str("]}");
                    }
                    out.push_str("]}");
                }
                ShapeContent::Chart(chart) => {
                    out.push_str(",\"chart\":");
                    write_chart(chart, out);
                }
            }
            out.push('}');
        }
        out.push_str("]}");
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

/// Is this paragraph a slide boundary - a single `slide-N` anchor?
/// (pptx, legacy ppt and odp all emit one per slide.)
fn slide_anchor(block: &Block) -> bool {
    let Block::Paragraph(inlines) = block else { return false };
    let [Inline::Anchor(id)] = inlines.as_slice() else { return false };
    id.strip_prefix("slide-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The `slide-N` id of a slide-boundary paragraph.
fn anchor_id(block: &Block) -> Option<&str> {
    let Block::Paragraph(inlines) = block else { return None };
    let [Inline::Anchor(id)] = inlines.as_slice() else { return None };
    Some(id)
}

/// In a spreadsheet document, a level-2 heading is BY CONSTRUCTION a sheet
/// name - the sheet converter emits nothing else at that level (cells become
/// table content, never headings). Returns the sheet's name.
fn sheet_heading(block: &Block) -> Option<String> {
    let Block::Heading { level: 2, content, .. } = block else { return None };
    let [Inline::Text { text, .. }] = content.as_slice() else { return None };
    Some(text.clone())
}

/// The whole document as IR v2 JSON plus the referenced asset bytes.
///
/// Boundary markers become PAGE boundaries - the IR's page list is the one
/// place the renderer already knows how to paginate:
///   - slide anchors (pptx/ppt/odp): one page per slide, unnamed (the app
///     labels them "Slide N" in the user's language);
///   - sheet-name headings (xlsx/ods): one page per sheet, NAMED - the tab
///     carries the name, so the heading block itself is consumed.
pub fn document_to_ir<'a>(doc: &'a Document, source_type: &str) -> IrOutput<'a> {
    let is_sheet = matches!(source_type, "xlsx" | "ods" | "csv");
    let layouts: std::collections::HashMap<&str, &SlideLayout> =
        doc.slides.iter().map(|s| (s.anchor.as_str(), s)).collect();
    let mut pages: Vec<(Option<String>, Vec<&Block>)> = vec![(None, Vec::new())];
    let mut page_slides: Vec<Option<&SlideLayout>> = vec![None];
    let mut seen_slide = false;
    for block in &doc.blocks {
        if slide_anchor(block) {
            if layouts.is_empty() {
                if !pages.last().unwrap().1.is_empty() {
                    pages.push((None, Vec::new()));
                    page_slides.push(None);
                }
            } else {
                // With fixed layouts every slide is its own page - one with
                // no text still has shapes and pictures to show.
                let layout = anchor_id(block).and_then(|id| layouts.get(id)).copied();
                if seen_slide || !pages.last().unwrap().1.is_empty() {
                    pages.push((None, Vec::new()));
                    page_slides.push(layout);
                } else {
                    *page_slides.last_mut().unwrap() = layout;
                }
                seen_slide = true;
            }
            continue;
        }
        if is_sheet && let Some(name) = sheet_heading(block) {
            // The first sheet's heading names the initial (still empty)
            // page; every later one starts the next page.
            let last = pages.last_mut().unwrap();
            if last.1.is_empty() && last.0.is_none() {
                last.0 = Some(name);
            } else {
                pages.push((Some(name), Vec::new()));
                page_slides.push(None);
            }
            continue;
        }
        pages.last_mut().unwrap().1.push(block);
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
    for (pi, (name, page)) in pages.iter().enumerate() {
        if pi > 0 {
            json.push(',');
        }
        json.push_str(&format!("{{\"pageIndex\":{pi},"));
        if let Some(name) = name {
            str_field(&mut json, "name", name);
            json.push(',');
        }
        json.push_str("\"blocks\":[");
        let mut first = true;
        for block in page {
            emitter.write_block(block, &mut json, &mut first);
        }
        json.push(']');
        if let Some(layout) = page_slides[pi] {
            json.push_str(",\"slide\":");
            emitter.write_slide(layout, &mut json);
        }
        json.push('}');
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
