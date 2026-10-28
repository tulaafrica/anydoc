//! TULA FORK: each slide's fixed layout - see [`SlideLayout`].
//!
//! The block walk in `mod.rs` answers "what does this slide say"; this
//! module answers "what does it look like". It reads every shape's position
//! and resolves what PowerPoint resolves before drawing: theme colours and
//! their modifiers, placeholder inheritance from the layout and master,
//! `p:style` references into the theme, table styles (including the
//! built-in default), autofit scaling, and SmartArt's saved drawing. It
//! shares nothing mutable with the block walk, so the Markdown output is
//! unaffected.
//!
//! Failures degrade: anything unreadable is left out of the layout, and a
//! slide whose layout cannot be built at all simply has none (the reader
//! falls back to the text view). Only resource-limit errors propagate.

use super::{LAYOUT_REL, MASTER_REL, SUPPORTED_NS};
use crate::error::ConvertError;
use crate::model::{
    Block, Crop, Fill, Frame, Geometry, ImageSource, Line, LineDash, MarkerKind, PathCmd, Rgba,
    Shadow, ShapeContent, SlideAlign, SlideBullet, SlideLayout, SlideParagraph, SlideRun,
    SlideShape, SlideTable, SlideTableCell, SlideTableRow, Spacing, TextAnchor, TextFrame,
};
use crate::package::Package;
use crate::package::path;
use crate::package::relationships::{Relationships, TargetMode, read_rels, rels_part_for};
use crate::package::xml::{Element, ns, parse_xml};
use crate::shared::assets::{AssetSink, rel_image_source};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const THEME_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
const TABLE_STYLES_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles";
/// SmartArt's saved drawing (`dsp:` shapes), written by PowerPoint 2007+.
const DSP: &str = "http://schemas.microsoft.com/office/drawing/2008/diagram";

const LEVELS: usize = 9;
const BLACK: Rgba = Rgba { rgb: [0, 0, 0], alpha: 1.0 };

// ------------------------------------------------------------------ parts ----

struct Part {
    path: String,
    tree: Element,
    rels: Relationships,
}

impl Part {
    fn view(&self) -> PartRef<'_> {
        PartRef { path: &self.path, tree: &self.tree, rels: &self.rels }
    }
}

/// A loaded part: its path (relationship base), XML, and relationships.
#[derive(Clone, Copy)]
struct PartRef<'p> {
    path: &'p str,
    tree: &'p Element,
    rels: &'p Relationships,
}

impl<'p> PartRef<'p> {
    /// The document element (`p:sld`, `p:sldLayout`, `p:sldMaster`, ...).
    fn root(&self) -> Option<&'p Element> {
        self.tree.child_elems().next()
    }

    fn sp_tree(&self) -> Option<&'p Element> {
        self.root()?.find(ns::P, "cSld")?.find(ns::P, "spTree")
    }

    fn rel_target(&self, rel_type: &str) -> Option<String> {
        let rel = self.rels.first_of_type(rel_type)?;
        path::resolve(self.path, &rel.target).ok().map(|t| t.path)
    }

    fn rel_path(&self, rid: &str) -> Option<String> {
        let rel = self.rels.get(rid)?;
        if rel.mode != TargetMode::Internal {
            return None;
        }
        path::resolve(self.path, &rel.target).ok().map(|t| t.path)
    }
}

// ------------------------------------------------------------ attributes ----

/// A number, accepting Strict's percentage form (`50%` = 50000).
fn parse_num(value: &str) -> Option<f64> {
    let value = value.trim();
    let n = match value.strip_suffix('%') {
        Some(pct) => pct.trim().parse::<f64>().ok()? * 1000.0,
        None => value.parse::<f64>().ok()?,
    };
    n.is_finite().then_some(n)
}

fn num(el: &Element, name: &str) -> Option<f64> {
    el.attr_unqualified(name).and_then(parse_num)
}

fn flag(el: &Element, name: &str) -> Option<bool> {
    el.attr_unqualified(name).map(|v| matches!(v, "1" | "true" | "on"))
}

/// A presentation-vocabulary child (`p:spPr`), or its SmartArt-drawing twin
/// (`dsp:spPr`) - the two share DrawingML content.
fn pchild<'e>(el: &'e Element, local: &str) -> Option<&'e Element> {
    el.child_elems()
        .find(|c| c.local == local && matches!(c.ns.as_deref(), Some(ns::P) | Some(DSP)))
}

fn is_hidden(el: &Element) -> bool {
    el.child_elems()
        .filter(|c| c.local.starts_with("nv"))
        .filter_map(|nv| pchild(nv, "cNvPr"))
        .any(|c| flag(c, "hidden") == Some(true))
}

fn placeholder_of(el: &Element) -> Option<&Element> {
    el.child_elems()
        .filter(|c| c.local.starts_with("nv"))
        .find_map(|nv| pchild(nv, "nvPr"))
        .and_then(|nv| nv.find(ns::P, "ph"))
}

// ----------------------------------------------------------------- colour ----

const DEFAULT_SCHEME: [(&str, [u8; 3]); 12] = [
    ("dk1", [0x00, 0x00, 0x00]),
    ("lt1", [0xFF, 0xFF, 0xFF]),
    ("dk2", [0x44, 0x54, 0x6A]),
    ("lt2", [0xE7, 0xE6, 0xE6]),
    ("accent1", [0x44, 0x72, 0xC4]),
    ("accent2", [0xED, 0x7D, 0x31]),
    ("accent3", [0xA5, 0xA5, 0xA5]),
    ("accent4", [0xFF, 0xC0, 0x00]),
    ("accent5", [0x5B, 0x9B, 0xD5]),
    ("accent6", [0x70, 0xAD, 0x47]),
    ("hlink", [0x05, 0x63, 0xC1]),
    ("folHlink", [0x95, 0x4F, 0x72]),
];

fn hex_rgb(value: &str) -> Option<[u8; 3]> {
    let value = value.trim();
    if value.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn preset_color(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        "silver" => [192, 192, 192],
        "gray" | "grey" => [128, 128, 128],
        "dkGray" | "darkGray" | "dkGrey" | "darkGrey" => [169, 169, 169],
        "ltGray" | "lightGray" | "ltGrey" | "lightGrey" => [211, 211, 211],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        _ => return None,
    })
}

fn rgb_to_hsl([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-9 {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, s, l]
}

fn hsl_to_rgb([h, s, l]: [f64; 3]) -> [f64; 3] {
    if s <= 0.0 {
        return [l, l, l];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0)]
}

struct Theme {
    scheme: HashMap<String, [u8; 3]>,
    major: Option<String>,
    minor: Option<String>,
    part: Option<Rc<Part>>,
}

impl Theme {
    fn new(part: Option<Rc<Part>>) -> Theme {
        let mut scheme: HashMap<String, [u8; 3]> =
            DEFAULT_SCHEME.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let mut major = Some("Calibri Light".to_string());
        let mut minor = Some("Calibri".to_string());
        if let Some(root) = part.as_ref().and_then(|p| p.view().root()) {
            if let Some(cs) = root.first_descendant(ns::A, "clrScheme") {
                for slot in cs.child_elems() {
                    let value = slot.child_elems().find_map(|c| match c.local.as_str() {
                        "srgbClr" => c.attr_unqualified("val").and_then(hex_rgb),
                        "sysClr" => c.attr_unqualified("lastClr").and_then(hex_rgb),
                        _ => None,
                    });
                    if let Some(v) = value {
                        scheme.insert(slot.local.clone(), v);
                    }
                }
            }
            if let Some(fs) = root.first_descendant(ns::A, "fontScheme") {
                let face = |kind: &str| {
                    fs.find(ns::A, kind)
                        .and_then(|f| f.find(ns::A, "latin"))
                        .and_then(|l| l.attr_unqualified("typeface"))
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                };
                major = face("majorFont").or(major);
                minor = face("minorFont").or(minor);
            }
        }
        Theme { scheme, major, minor, part }
    }

    /// The `i`-th (1-based) entry of a format-scheme list
    /// (`fillStyleLst`, `lnStyleLst`, `effectStyleLst`, `bgFillStyleLst`).
    fn style_entry(&self, list: &str, i: usize) -> Option<&Element> {
        let root = self.part.as_ref()?.tree.child_elems().next()?;
        let lst = root.first_descendant(ns::A, list)?;
        lst.child_elems().nth(i.checked_sub(1)?)
    }

    /// Resolve theme font references; empty names mean "unset".
    fn font(&self, face: &str) -> Option<String> {
        match face {
            "" => None,
            f if f.starts_with("+mj") => self.major.clone(),
            f if f.starts_with("+mn") => self.minor.clone(),
            f => Some(f.to_string()),
        }
    }
}

struct Colors {
    theme: Rc<Theme>,
    /// `p:clrMap` (bg1 -> lt1, tx1 -> dk1, ...), with overrides applied.
    map: HashMap<String, String>,
}

impl Colors {
    /// Resolve one colour element, modifiers included. `ph` stands in for
    /// `phClr` (the colour a style reference supplies).
    fn resolve(&self, el: &Element, ph: Option<Rgba>) -> Option<Rgba> {
        let val = el.attr_unqualified("val");
        let (rgb, mut alpha) = match el.local.as_str() {
            "srgbClr" => (hex_rgb(val?)?, 1.0),
            "schemeClr" => {
                let name = val?;
                if name == "phClr" {
                    let ph = ph?;
                    (ph.rgb, ph.alpha as f64)
                } else {
                    let mapped = self.map.get(name).map(String::as_str).unwrap_or(name);
                    (*self.theme.scheme.get(mapped).or_else(|| self.theme.scheme.get(name))?, 1.0)
                }
            }
            "sysClr" => {
                let last = el.attr_unqualified("lastClr").and_then(hex_rgb);
                let fallback = if val == Some("window") { [255, 255, 255] } else { [0, 0, 0] };
                (last.unwrap_or(fallback), 1.0)
            }
            "prstClr" => (preset_color(val?)?, 1.0),
            "scrgbClr" => {
                let c = |k: &str| (num(el, k).unwrap_or(0.0) / 100000.0).clamp(0.0, 1.0);
                let to8 = |v: f64| (v * 255.0).round() as u8;
                ([to8(c("r")), to8(c("g")), to8(c("b"))], 1.0)
            }
            "hslClr" => {
                let h = num(el, "hue").unwrap_or(0.0) / 60000.0 / 360.0;
                let s = num(el, "sat").unwrap_or(0.0) / 100000.0;
                let l = num(el, "lum").unwrap_or(0.0) / 100000.0;
                let [r, g, b] = hsl_to_rgb([h, s, l]);
                let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                ([to8(r), to8(g), to8(b)], 1.0)
            }
            _ => return None,
        };
        let mut c = [rgb[0] as f64 / 255.0, rgb[1] as f64 / 255.0, rgb[2] as f64 / 255.0];
        for m in el.child_elems() {
            let v = num(m, "val").unwrap_or(0.0) / 100000.0;
            match m.local.as_str() {
                "lumMod" | "lumOff" | "satMod" | "satOff" | "hueOff" | "hueMod" => {
                    let mut hsl = rgb_to_hsl(c);
                    match m.local.as_str() {
                        "lumMod" => hsl[2] *= v,
                        "lumOff" => hsl[2] += v,
                        "satMod" => hsl[1] *= v,
                        "satOff" => hsl[1] += v,
                        // hueOff's unit is 60000ths of a degree, not percent.
                        "hueOff" => hsl[0] += num(m, "val").unwrap_or(0.0) / 60000.0 / 360.0,
                        _ => hsl[0] *= v,
                    }
                    hsl[0] = hsl[0].rem_euclid(1.0);
                    hsl[1] = hsl[1].clamp(0.0, 1.0);
                    hsl[2] = hsl[2].clamp(0.0, 1.0);
                    c = hsl_to_rgb(hsl);
                }
                "tint" => c = c.map(|x| x + (1.0 - x) * (1.0 - v)),
                "shade" => c = c.map(|x| x * v),
                "alpha" => alpha = v,
                "alphaMod" => alpha *= v,
                "alphaOff" => alpha += v,
                "inv" => c = c.map(|x| 1.0 - x),
                "gray" => {
                    let g = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
                    c = [g, g, g];
                }
                _ => {}
            }
        }
        let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Rgba { rgb: [to8(c[0]), to8(c[1]), to8(c[2])], alpha: alpha.clamp(0.0, 1.0) as f32 })
    }

    /// The first child of `parent` that resolves as a colour.
    fn of(&self, parent: Option<&Element>, ph: Option<Rgba>) -> Option<Rgba> {
        parent?.child_elems().find_map(|c| self.resolve(c, ph))
    }

    fn scheme(&self, name: &str) -> Option<Rgba> {
        let mapped = self.map.get(name).map(String::as_str).unwrap_or(name);
        let rgb = *self.theme.scheme.get(mapped)?;
        Some(Rgba { rgb, alpha: 1.0 })
    }
}

// ------------------------------------------------------------ text styles ----

#[derive(Debug, Clone, PartialEq)]
enum BulletKind {
    None,
    Char(String),
    AutoNum { kind: MarkerKind, wrap: &'static str, start: u64 },
}

/// One run-property layer; `None` inherits.
#[derive(Debug, Clone, Default)]
struct RunProps {
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    caps: Option<bool>,
    color: Option<Rgba>,
    font: Option<String>,
    spacing: Option<f64>,
    baseline: Option<f64>,
}

impl RunProps {
    fn merge(&mut self, o: &RunProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(size, bold, italic, underline, strike, caps, color, font, spacing, baseline);
    }
}

/// One paragraph-level layer (`lvlNpPr` / `pPr`); `None` inherits.
#[derive(Debug, Clone, Default)]
struct LvlProps {
    align: Option<SlideAlign>,
    margin_left: Option<f64>,
    indent: Option<f64>,
    line_spacing: Option<Spacing>,
    space_before: Option<Spacing>,
    space_after: Option<Spacing>,
    bullet: Option<BulletKind>,
    /// `Some(None)`: follow the text (`buClrTx`).
    bullet_color: Option<Option<Rgba>>,
    bullet_font: Option<Option<String>>,
    bullet_size: Option<f64>,
    run: RunProps,
}

impl LvlProps {
    fn merge(&mut self, o: &LvlProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            align,
            margin_left,
            indent,
            line_spacing,
            space_before,
            space_after,
            bullet,
            bullet_color,
            bullet_font,
            bullet_size
        );
        self.run.merge(&o.run);
    }
}

type LvlSet = Vec<LvlProps>;

fn empty_set() -> LvlSet {
    vec![LvlProps::default(); LEVELS]
}

fn spacing(el: Option<&Element>) -> Option<Spacing> {
    let el = el?;
    if let Some(pct) = el.find(ns::A, "spcPct") {
        return num(pct, "val").map(|v| Spacing::Lines(v / 100000.0));
    }
    el.find(ns::A, "spcPts").and_then(|p| num(p, "val")).map(Spacing::Points)
}

fn rpr_props(rpr: &Element, colors: &Colors) -> RunProps {
    let mut out = RunProps {
        size: num(rpr, "sz"),
        bold: flag(rpr, "b"),
        italic: flag(rpr, "i"),
        underline: rpr.attr_unqualified("u").map(|v| v != "none"),
        strike: rpr.attr_unqualified("strike").map(|v| v != "noStrike"),
        caps: rpr.attr_unqualified("cap").map(|v| v != "none"),
        spacing: num(rpr, "spc"),
        baseline: num(rpr, "baseline").map(|v| v / 100000.0),
        ..RunProps::default()
    };
    for c in rpr.child_elems() {
        match c.local.as_str() {
            "solidFill" => out.color = colors.of(Some(c), None).or(out.color),
            "gradFill" => {
                out.color = c
                    .find(ns::A, "gsLst")
                    .and_then(|l| l.find(ns::A, "gs"))
                    .and_then(|gs| colors.of(Some(gs), None))
                    .or(out.color)
            }
            "noFill" => out.color = Some(Rgba { rgb: [0, 0, 0], alpha: 0.0 }),
            "latin" => {
                if let Some(face) = c.attr_unqualified("typeface") {
                    out.font = colors.theme.font(face).or(out.font);
                }
            }
            _ => {}
        }
    }
    out
}

fn ppr_props(ppr: &Element, colors: &Colors) -> LvlProps {
    let mut out = LvlProps {
        align: ppr.attr_unqualified("algn").map(|v| match v {
            "ctr" => SlideAlign::Center,
            "r" => SlideAlign::Right,
            "just" | "justLow" | "dist" | "thaiDist" => SlideAlign::Justify,
            _ => SlideAlign::Left,
        }),
        margin_left: num(ppr, "marL"),
        indent: num(ppr, "indent"),
        line_spacing: spacing(ppr.find(ns::A, "lnSpc")),
        space_before: spacing(ppr.find(ns::A, "spcBef")),
        space_after: spacing(ppr.find(ns::A, "spcAft")),
        ..LvlProps::default()
    };
    for c in ppr.child_elems() {
        match c.local.as_str() {
            "buNone" => out.bullet = Some(BulletKind::None),
            "buChar" => {
                out.bullet =
                    Some(BulletKind::Char(c.attr_unqualified("char").unwrap_or("•").to_string()))
            }
            "buBlip" => out.bullet = Some(BulletKind::Char("•".to_string())),
            "buAutoNum" => {
                let scheme = c.attr_unqualified("type").unwrap_or("arabicPeriod");
                let kind = match scheme {
                    s if s.starts_with("alphaLc") => MarkerKind::LowerAlpha,
                    s if s.starts_with("alphaUc") => MarkerKind::UpperAlpha,
                    s if s.starts_with("romanLc") => MarkerKind::LowerRoman,
                    s if s.starts_with("romanUc") => MarkerKind::UpperRoman,
                    _ => MarkerKind::Decimal,
                };
                let wrap = match scheme {
                    s if s.ends_with("ParenBoth") => "()",
                    s if s.ends_with("ParenR") => ")",
                    s if s.ends_with("Plain") => "",
                    s if s.ends_with("Minus") => "-",
                    _ => ".",
                };
                let start = num(c, "startAt").map(|v| v.clamp(1.0, 32767.0) as u64).unwrap_or(1);
                out.bullet = Some(BulletKind::AutoNum { kind, wrap, start });
            }
            "buClr" => out.bullet_color = Some(colors.of(Some(c), None)),
            "buClrTx" => out.bullet_color = Some(None),
            "buFont" => {
                out.bullet_font =
                    Some(c.attr_unqualified("typeface").and_then(|f| colors.theme.font(f)))
            }
            "buFontTx" => out.bullet_font = Some(None),
            "buSzPct" => out.bullet_size = num(c, "val").map(|v| v / 100000.0),
            "buSzTx" => out.bullet_size = Some(1.0),
            "defRPr" => out.run = rpr_props(c, colors),
            _ => {}
        }
    }
    out
}

fn level_set(lst: Option<&Element>, colors: &Colors) -> LvlSet {
    let mut set = empty_set();
    if let Some(lst) = lst {
        for (i, slot) in set.iter_mut().enumerate() {
            if let Some(ppr) = lst.find(ns::A, &format!("lvl{}pPr", i + 1)) {
                *slot = ppr_props(ppr, colors);
            }
        }
    }
    set
}

/// Symbol-font bullets name glyphs by their slot in a legacy font the phone
/// does not have; map the common ones to their Unicode equivalents.
fn bullet_glyph(text: &str, font: Option<&str>) -> (String, bool) {
    let symbolic = font.is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.starts_with("wingdings") || f == "symbol" || f.starts_with("webdings")
    });
    if !symbolic {
        return (text.to_string(), false);
    }
    let mapped = match text {
        "l" | "\u{F06C}" => "●",
        "n" | "§" | "\u{F06E}" | "\u{F0A7}" => "■",
        "q" | "\u{F071}" => "❑",
        "v" | "\u{F076}" => "❖",
        "Ø" | "\u{F0D8}" => "➢",
        "ü" | "\u{F0FC}" => "✔",
        "è" | "\u{F0E8}" => "➔",
        "Ÿ" | "\u{F09F}" => "•",
        "o" => "○",
        _ => "•",
    };
    (mapped.to_string(), true)
}

/// Run text: vertical tabs (PowerPoint's soft breaks) and newlines become
/// line breaks; other control characters go.
fn run_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\u{b}' => out.push('\n'),
            '\t' => out.push('\t'),
            '\u{ad}' | '\u{feff}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// What a text body inherits before its own `lstStyle`.
struct TextInherit<'a> {
    /// Level sets, outermost first.
    chain: Vec<&'a LvlSet>,
    /// Run overrides applied after the chain (style `fontRef` colour, table
    /// style text).
    over: RunProps,
}

/// Body properties gathered from the inheritance chain, outermost first.
fn body_props(chain: &[Option<&Element>]) -> ([f64; 4], TextAnchor, bool) {
    let get = |name: &str| chain.iter().rev().flatten().find_map(|b| b.attr_unqualified(name));
    let inset = |name: &str, default: f64| get(name).and_then(parse_num).unwrap_or(default);
    let insets = [
        inset("lIns", 91440.0),
        inset("tIns", 45720.0),
        inset("rIns", 91440.0),
        inset("bIns", 45720.0),
    ];
    let anchor = match get("anchor") {
        Some("ctr") => TextAnchor::Middle,
        Some("b") => TextAnchor::Bottom,
        _ => TextAnchor::Top,
    };
    let wrap = get("wrap") != Some("none");
    (insets, anchor, wrap)
}

fn paragraphs(
    tx: &Element,
    inherit: &TextInherit,
    colors: &Colors,
    font_scale: f64,
    line_reduction: f64,
) -> Vec<SlideParagraph> {
    let own = level_set(tx.find(ns::A, "lstStyle"), colors);
    let mut counters = [0u64; LEVELS];
    let mut started = [false; LEVELS];
    let mut out = Vec::new();
    for p in tx.find_all(ns::A, "p") {
        let ppr = p.find(ns::A, "pPr");
        let lvl =
            ppr.and_then(|pr| num(pr, "lvl")).map(|v| v.clamp(0.0, 8.0) as usize).unwrap_or(0);
        let mut lp = LvlProps::default();
        for set in &inherit.chain {
            lp.merge(&set[lvl]);
        }
        lp.run.merge(&inherit.over);
        lp.merge(&own[lvl]);
        if let Some(ppr) = ppr {
            lp.merge(&ppr_props(ppr, colors));
        }

        let mut runs: Vec<SlideRun> = Vec::new();
        collect_runs(p, &lp.run, colors, font_scale, &mut runs);
        let has_text = runs.iter().any(|r| !r.text.trim().is_empty());

        let bullet = match (&lp.bullet, has_text) {
            (Some(BulletKind::Char(c)), true) => Some(c.clone()),
            (Some(BulletKind::AutoNum { kind, wrap, start }), true) => {
                let n = if started[lvl] {
                    counters[lvl].saturating_add(1)
                } else {
                    started[lvl] = true;
                    *start
                };
                counters[lvl] = n;
                let ordinal = kind.ordinal(n);
                Some(match *wrap {
                    "()" => format!("({ordinal})"),
                    w => format!("{ordinal}{w}"),
                })
            }
            _ => None,
        };
        // A shallower paragraph restarts deeper numbering; an unnumbered one
        // restarts its own level.
        if has_text {
            let numbered = matches!(lp.bullet, Some(BulletKind::AutoNum { .. }));
            let from = if numbered { lvl + 1 } else { lvl };
            for s in started.iter_mut().skip(from) {
                *s = false;
            }
        }
        let bullet = bullet.map(|text| {
            let font = lp.bullet_font.clone().flatten();
            let (text, dropped) = bullet_glyph(&text, font.as_deref());
            SlideBullet {
                text,
                color: lp.bullet_color.flatten(),
                font: if dropped { None } else { font },
                size: lp.bullet_size.unwrap_or(1.0),
            }
        });

        let end_size = p
            .find(ns::A, "endParaRPr")
            .and_then(|e| num(e, "sz"))
            .or(lp.run.size)
            .unwrap_or(1800.0)
            * font_scale;
        let reduce = |s: Option<Spacing>| match s {
            _ if line_reduction <= 0.0 => s,
            Some(Spacing::Lines(v)) => Some(Spacing::Lines((v - line_reduction).max(0.1))),
            Some(Spacing::Points(v)) => Some(Spacing::Points(v * (1.0 - line_reduction))),
            None => Some(Spacing::Lines((1.0 - line_reduction).max(0.1))),
        };
        out.push(SlideParagraph {
            align: lp.align.unwrap_or(SlideAlign::Left),
            margin_left: lp.margin_left.unwrap_or(0.0),
            indent: lp.indent.unwrap_or(0.0),
            line_spacing: reduce(lp.line_spacing),
            // PowerPoint does not put space above a text body's first
            // paragraph; the box's top inset is the only gap there.
            space_before: if out.is_empty() { None } else { lp.space_before },
            space_after: lp.space_after,
            bullet,
            runs,
            end_size,
        });
    }
    out
}

fn collect_runs(
    container: &Element,
    base: &RunProps,
    colors: &Colors,
    font_scale: f64,
    runs: &mut Vec<SlideRun>,
) {
    for child in container.child_elems() {
        if child.is(ns::MC, "AlternateContent") {
            if let Some(branch) = crate::shared::mc::alternate_branch(child, SUPPORTED_NS) {
                collect_runs(branch, base, colors, font_scale, runs);
            }
            continue;
        }
        if child.ns.as_deref() != Some(ns::A) {
            continue;
        }
        let (text, rpr) = match child.local.as_str() {
            "r" | "fld" => (
                run_text(&child.find(ns::A, "t").map(|t| t.text()).unwrap_or_default()),
                child.find(ns::A, "rPr"),
            ),
            "br" => ("\n".to_string(), child.find(ns::A, "rPr")),
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        let mut rp = base.clone();
        if let Some(rpr) = rpr {
            let own = rpr_props(rpr, colors);
            // Hyperlinks take the theme's link colour and an underline unless
            // the run says otherwise.
            if rpr.find(ns::A, "hlinkClick").is_some() {
                if own.color.is_none() {
                    rp.color = colors.scheme("hlink");
                }
                if own.underline.is_none() {
                    rp.underline = Some(true);
                }
            }
            rp.merge(&own);
        }
        runs.push(SlideRun {
            text,
            size: rp.size.unwrap_or(1800.0) * font_scale,
            font: rp.font,
            bold: rp.bold.unwrap_or(false),
            italic: rp.italic.unwrap_or(false),
            underline: rp.underline.unwrap_or(false),
            strike: rp.strike.unwrap_or(false),
            color: rp.color.unwrap_or(BLACK),
            letter_spacing: rp.spacing.unwrap_or(0.0),
            baseline: rp.baseline.unwrap_or(0.0),
            all_caps: rp.caps.unwrap_or(false),
        });
    }
}

// --------------------------------------------------------------- geometry ----

/// Group coordinate mapping: child space (`chOff`/`chExt`) to slide space.
#[derive(Clone, Copy)]
struct Tf {
    ox: f64,
    oy: f64,
    sx: f64,
    sy: f64,
}

impl Tf {
    const IDENTITY: Tf = Tf { ox: 0.0, oy: 0.0, sx: 1.0, sy: 1.0 };

    fn frame(&self, f: Frame) -> Frame {
        Frame {
            x: self.ox + f.x * self.sx,
            y: self.oy + f.y * self.sy,
            width: f.width * self.sx,
            height: f.height * self.sy,
            ..f
        }
    }

    fn child(&self, xfrm: &Element) -> Tf {
        let pair = |name: &str, a: &str, b: &str| {
            xfrm.find(ns::A, name).map(|e| (num(e, a).unwrap_or(0.0), num(e, b).unwrap_or(0.0)))
        };
        let (Some(off), Some(ext), Some(ch_off), Some(ch_ext)) = (
            pair("off", "x", "y"),
            pair("ext", "cx", "cy"),
            pair("chOff", "x", "y"),
            pair("chExt", "cx", "cy"),
        ) else {
            return *self;
        };
        let kx = if ch_ext.0.abs() > 0.0 { ext.0 / ch_ext.0 } else { 1.0 };
        let ky = if ch_ext.1.abs() > 0.0 { ext.1 / ch_ext.1 } else { 1.0 };
        Tf {
            ox: self.ox + (off.0 - ch_off.0 * kx) * self.sx,
            oy: self.oy + (off.1 - ch_off.1 * ky) * self.sy,
            sx: self.sx * kx,
            sy: self.sy * ky,
        }
    }
}

fn read_xfrm(xfrm: Option<&Element>) -> Option<Frame> {
    let xfrm = xfrm?;
    let off = xfrm.find(ns::A, "off")?;
    let ext = xfrm.find(ns::A, "ext");
    Some(Frame {
        x: num(off, "x").unwrap_or(0.0),
        y: num(off, "y").unwrap_or(0.0),
        width: ext.and_then(|e| num(e, "cx")).unwrap_or(0.0),
        height: ext.and_then(|e| num(e, "cy")).unwrap_or(0.0),
        rotation: num(xfrm, "rot").unwrap_or(0.0) / 60000.0,
        flip_h: flag(xfrm, "flipH") == Some(true),
        flip_v: flag(xfrm, "flipV") == Some(true),
    })
}

/// SVG-like path builder in frame space.
#[derive(Default)]
struct PathB(Vec<PathCmd>);

impl PathB {
    fn m(&mut self, x: f64, y: f64) -> &mut Self {
        self.0.push(PathCmd::MoveTo(x, y));
        self
    }
    fn l(&mut self, x: f64, y: f64) -> &mut Self {
        self.0.push(PathCmd::LineTo(x, y));
        self
    }
    fn a(&mut self, rx: f64, ry: f64, large: bool, sweep: bool, x: f64, y: f64) -> &mut Self {
        if rx <= 0.0 || ry <= 0.0 {
            return self.l(x, y);
        }
        self.0.push(PathCmd::ArcTo { rx, ry, large, sweep, x, y });
        self
    }
    fn z(&mut self) -> &mut Self {
        self.0.push(PathCmd::Close);
        self
    }
    fn poly(&mut self, pts: &[(f64, f64)]) -> &mut Self {
        for (i, (x, y)) in pts.iter().enumerate() {
            if i == 0 {
                self.m(*x, *y);
            } else {
                self.l(*x, *y);
            }
        }
        self.z()
    }
    /// An arc of the ellipse centred (cx, cy) from `start` sweeping `sweep`
    /// degrees (clockwise positive), assuming the pen is at the start point.
    fn ellipse_arc(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, start: f64, sweep: f64) {
        let pt = |deg: f64| {
            let r = deg.to_radians();
            (cx + rx * r.cos(), cy + ry * r.sin())
        };
        // More than a full turn draws nothing new (and hostile angles must
        // not drive the loop); SVG cannot draw a full turn in one arc, so
        // split what remains.
        let sweep = sweep.clamp(-360.0, 360.0);
        let pieces = (sweep.abs() / 180.0).ceil().max(1.0) as usize;
        let step = sweep / pieces as f64;
        for i in 1..=pieces {
            let (x, y) = pt(start + step * i as f64);
            self.a(rx, ry, false, step > 0.0, x, y);
        }
    }
    fn done(self, filled: bool) -> Geometry {
        Geometry::Path { commands: self.0, filled }
    }
}

/// Polygon vertices of a regular figure, scaled to fill the w x h box.
fn fitted(points: &[(f64, f64)], w: f64, h: f64) -> Vec<(f64, f64)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    let (sw, sh) = ((x1 - x0).max(1e-9), (y1 - y0).max(1e-9));
    points.iter().map(|(x, y)| ((x - x0) / sw * w, (y - y0) / sh * h)).collect()
}

fn star(n: usize, inner: f64, w: f64, h: f64) -> Vec<(f64, f64)> {
    let mut pts = Vec::with_capacity(n * 2);
    for i in 0..n * 2 {
        let r = if i % 2 == 0 { 1.0 } else { inner };
        let a = (-90.0 + 180.0 / n as f64 * i as f64).to_radians();
        pts.push((r * a.cos(), r * a.sin()));
    }
    fitted(&pts, w, h)
}

fn regular(n: usize, w: f64, h: f64) -> Vec<(f64, f64)> {
    let pts: Vec<_> = (0..n)
        .map(|i| {
            let a = (-90.0 + 360.0 / n as f64 * i as f64).to_radians();
            (a.cos(), a.sin())
        })
        .collect();
    fitted(&pts, w, h)
}

/// A preset outline. Unknown presets draw as their frame rectangle.
fn preset_geometry(name: &str, adj: &HashMap<String, f64>, w: f64, h: f64) -> Geometry {
    let ss = w.min(h);
    let a = |key: &str, default: f64| adj.get(key).copied().unwrap_or(default);
    let mut p = PathB::default();
    match name {
        "rect" | "flowChartProcess" => Geometry::Rect,
        "roundRect" => {
            Geometry::RoundRect { radius: (a("adj", 16667.0) / 100000.0).clamp(0.0, 0.5) }
        }
        "flowChartAlternateProcess" => Geometry::RoundRect { radius: 1.0 / 6.0 },
        "ellipse" | "flowChartConnector" => Geometry::Ellipse,
        "line" | "straightConnector1" => Geometry::Line,
        "bentConnector2" => {
            p.m(0.0, 0.0).l(w, 0.0).l(w, h);
            p.done(false)
        }
        "bentConnector3" => {
            let x1 = w * a("adj1", 50000.0) / 100000.0;
            p.m(0.0, 0.0).l(x1, 0.0).l(x1, h).l(w, h);
            p.done(false)
        }
        "curvedConnector3" => {
            let x1 = w * a("adj1", 50000.0) / 100000.0;
            p.m(0.0, 0.0);
            p.0.push(PathCmd::CubicTo(x1, 0.0, x1, h, w, h));
            p.done(false)
        }
        "triangle" | "flowChartExtract" => {
            let x = w * a("adj", 50000.0) / 100000.0;
            p.poly(&[(x, 0.0), (w, h), (0.0, h)]);
            p.done(true)
        }
        "rtTriangle" => {
            p.poly(&[(0.0, 0.0), (w, h), (0.0, h)]);
            p.done(true)
        }
        "diamond" | "flowChartDecision" => {
            p.poly(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)]);
            p.done(true)
        }
        "parallelogram" | "flowChartInputOutput" => {
            let x =
                if name == "parallelogram" { ss * a("adj", 25000.0) / 100000.0 } else { w / 5.0 };
            p.poly(&[(0.0, h), (x, 0.0), (w, 0.0), (w - x, h)]);
            p.done(true)
        }
        "trapezoid" => {
            let x = ss * a("adj", 25000.0) / 100000.0;
            p.poly(&[(0.0, h), (x, 0.0), (w - x, 0.0), (w, h)]);
            p.done(true)
        }
        "pentagon" => {
            p.poly(&regular(5, w, h));
            p.done(true)
        }
        "hexagon" => {
            let x = ss * a("adj", 25000.0) / 100000.0;
            p.poly(&[(0.0, h / 2.0), (x, 0.0), (w - x, 0.0), (w, h / 2.0), (w - x, h), (x, h)]);
            p.done(true)
        }
        "heptagon" => {
            p.poly(&regular(7, w, h));
            p.done(true)
        }
        "octagon" => {
            let x = ss * a("adj", 29289.0) / 100000.0;
            p.poly(&[
                (0.0, x),
                (x, 0.0),
                (w - x, 0.0),
                (w, x),
                (w, h - x),
                (w - x, h),
                (x, h),
                (0.0, h - x),
            ]);
            p.done(true)
        }
        "decagon" => {
            p.poly(&regular(10, w, h));
            p.done(true)
        }
        "homePlate" => {
            let x = w - ss * a("adj", 50000.0) / 100000.0;
            p.poly(&[(0.0, 0.0), (x, 0.0), (w, h / 2.0), (x, h), (0.0, h)]);
            p.done(true)
        }
        "chevron" => {
            let x = ss * a("adj", 50000.0) / 100000.0;
            p.poly(&[(0.0, 0.0), (w - x, 0.0), (w, h / 2.0), (w - x, h), (0.0, h), (x, h / 2.0)]);
            p.done(true)
        }
        "rightArrow" | "leftArrow" | "leftRightArrow" => {
            let dy = h * a("adj1", 50000.0) / 200000.0;
            let (y1, y2) = (h / 2.0 - dy, h / 2.0 + dy);
            let head = ss * a("adj2", 50000.0) / 100000.0;
            match name {
                "rightArrow" => p.poly(&[
                    (0.0, y1),
                    (w - head, y1),
                    (w - head, 0.0),
                    (w, h / 2.0),
                    (w - head, h),
                    (w - head, y2),
                    (0.0, y2),
                ]),
                "leftArrow" => p.poly(&[
                    (0.0, h / 2.0),
                    (head, 0.0),
                    (head, y1),
                    (w, y1),
                    (w, y2),
                    (head, y2),
                    (head, h),
                ]),
                _ => p.poly(&[
                    (0.0, h / 2.0),
                    (head, 0.0),
                    (head, y1),
                    (w - head, y1),
                    (w - head, 0.0),
                    (w, h / 2.0),
                    (w - head, h),
                    (w - head, y2),
                    (head, y2),
                    (head, h),
                ]),
            };
            p.done(true)
        }
        "upArrow" | "downArrow" => {
            let dx = w * a("adj1", 50000.0) / 200000.0;
            let (x1, x2) = (w / 2.0 - dx, w / 2.0 + dx);
            let head = ss * a("adj2", 50000.0) / 100000.0;
            if name == "upArrow" {
                p.poly(&[
                    (w / 2.0, 0.0),
                    (w, head),
                    (x2, head),
                    (x2, h),
                    (x1, h),
                    (x1, head),
                    (0.0, head),
                ]);
            } else {
                p.poly(&[
                    (x1, 0.0),
                    (x2, 0.0),
                    (x2, h - head),
                    (w, h - head),
                    (w / 2.0, h),
                    (0.0, h - head),
                    (x1, h - head),
                ]);
            }
            p.done(true)
        }
        "plus" | "flowChartOr" if name == "plus" => {
            let x = ss * a("adj", 25000.0) / 100000.0;
            p.poly(&[
                (0.0, x),
                (x, x),
                (x, 0.0),
                (w - x, 0.0),
                (w - x, x),
                (w, x),
                (w, h - x),
                (w - x, h - x),
                (w - x, h),
                (x, h),
                (x, h - x),
                (0.0, h - x),
            ]);
            p.done(true)
        }
        "star4" | "star5" | "star6" | "star7" | "star8" | "star10" | "star12" => {
            let (n, default) = match name {
                "star4" => (4, 12500.0),
                "star5" => (5, 19098.0),
                "star6" => (6, 28868.0),
                "star7" => (7, 34601.0),
                "star8" => (8, 37500.0),
                "star10" => (10, 42533.0),
                _ => (12, 37500.0),
            };
            p.poly(&star(n, (a("adj", default) / 50000.0).clamp(0.05, 1.0), w, h));
            p.done(true)
        }
        "snip1Rect" => {
            let x = ss * a("adj", 16667.0) / 100000.0;
            p.poly(&[(0.0, 0.0), (w - x, 0.0), (w, x), (w, h), (0.0, h)]);
            p.done(true)
        }
        "snip2SameRect" => {
            let t = ss * a("adj1", 16667.0) / 100000.0;
            let b = ss * a("adj2", 0.0) / 100000.0;
            p.poly(&[
                (t, 0.0),
                (w - t, 0.0),
                (w, t),
                (w, h - b),
                (w - b, h),
                (b, h),
                (0.0, h - b),
                (0.0, t),
            ]);
            p.done(true)
        }
        "round1Rect" => {
            let r = ss * a("adj", 16667.0) / 100000.0;
            p.m(0.0, 0.0).l(w - r, 0.0).a(r, r, false, true, w, r).l(w, h).l(0.0, h).z();
            p.done(true)
        }
        "round2SameRect" => {
            let t = ss * a("adj1", 16667.0) / 100000.0;
            let b = ss * a("adj2", 0.0) / 100000.0;
            p.m(t, 0.0)
                .l(w - t, 0.0)
                .a(t, t, false, true, w, t)
                .l(w, h - b)
                .a(b, b, false, true, w - b, h)
                .l(b, h)
                .a(b, b, false, true, 0.0, h - b)
                .l(0.0, t)
                .a(t, t, false, true, t, 0.0)
                .z();
            p.done(true)
        }
        "round2DiagRect" => {
            let r1 = ss * a("adj1", 16667.0) / 100000.0;
            let r2 = ss * a("adj2", 0.0) / 100000.0;
            p.m(r1, 0.0)
                .l(w - r2, 0.0)
                .a(r2, r2, false, true, w, r2)
                .l(w, h - r1)
                .a(r1, r1, false, true, w - r1, h)
                .l(r2, h)
                .a(r2, r2, false, true, 0.0, h - r2)
                .l(0.0, r1)
                .a(r1, r1, false, true, r1, 0.0)
                .z();
            p.done(true)
        }
        "flowChartTerminator" => {
            let rx = w * 3475.0 / 21600.0;
            let ry = h / 2.0;
            p.m(rx, 0.0)
                .l(w - rx, 0.0)
                .a(rx, ry, false, true, w - rx, h)
                .l(rx, h)
                .a(rx, ry, false, true, rx, 0.0)
                .z();
            p.done(true)
        }
        "donut" => {
            let t = (ss * a("adj", 25000.0) / 100000.0).min(ss / 2.0);
            let (rx, ry) = (w / 2.0, h / 2.0);
            p.m(0.0, ry);
            p.ellipse_arc(rx, ry, rx, ry, 180.0, 360.0);
            p.z();
            // The inner ring runs the other way, cutting the hole.
            p.m(t, ry);
            p.ellipse_arc(rx, ry, rx - t, ry - t, 180.0, -360.0);
            p.z();
            p.done(true)
        }
        "arc" | "pie" | "blockArc" | "chord" => {
            let (d1, d2) = match name {
                "arc" => (16200000.0, 0.0),
                "pie" => (0.0, 16200000.0),
                "chord" => (2700000.0, 16200000.0),
                _ => (10800000.0, 0.0),
            };
            let start = a("adj1", d1) / 60000.0;
            let end = a("adj2", d2) / 60000.0;
            let sweep = (end - start).rem_euclid(360.0);
            let sweep = if sweep == 0.0 { 360.0 } else { sweep };
            let (cx, cy, rx, ry) = (w / 2.0, h / 2.0, w / 2.0, h / 2.0);
            let at = |deg: f64, rx: f64, ry: f64| {
                let r = deg.to_radians();
                (cx + rx * r.cos(), cy + ry * r.sin())
            };
            let (sx, sy) = at(start, rx, ry);
            match name {
                "arc" => {
                    p.m(sx, sy);
                    p.ellipse_arc(cx, cy, rx, ry, start, sweep);
                    p.done(false)
                }
                "pie" => {
                    p.m(cx, cy).l(sx, sy);
                    p.ellipse_arc(cx, cy, rx, ry, start, sweep);
                    p.z();
                    p.done(true)
                }
                "chord" => {
                    p.m(sx, sy);
                    p.ellipse_arc(cx, cy, rx, ry, start, sweep);
                    p.z();
                    p.done(true)
                }
                _ => {
                    let t = (ss * a("adj3", 25000.0) / 100000.0).min(ss / 2.0);
                    let (irx, iry) = (rx - t, ry - t);
                    p.m(sx, sy);
                    p.ellipse_arc(cx, cy, rx, ry, start, sweep);
                    let (ex, ey) = at(start + sweep, irx, iry);
                    p.l(ex, ey);
                    p.ellipse_arc(cx, cy, irx, iry, start + sweep, -sweep);
                    p.z();
                    p.done(true)
                }
            }
        }
        "wedgeRectCallout" | "wedgeRoundRectCallout" | "wedgeEllipseCallout" => {
            // ECMA-376 presetShapeDefinitions: the tip sits at the centre plus
            // (adj1, adj2); the tail leaves the edge facing it, its base
            // spanning 2/12-5/12 (tip before the centre) or 7/12-10/12.
            let dx = w * a("adj1", -20833.0) / 100000.0;
            let dy = h * a("adj2", 62500.0) / 100000.0;
            let (tx, ty) = (w / 2.0 + dx, h / 2.0 + dy);
            let vertical = dy.abs() - (dx * h / w.max(1e-9)).abs() > 0.0;
            let (bx1, bx2) = if dx > 0.0 {
                (w * 7.0 / 12.0, w * 10.0 / 12.0)
            } else {
                (w * 2.0 / 12.0, w * 5.0 / 12.0)
            };
            let (by1, by2) = if dy > 0.0 {
                (h * 7.0 / 12.0, h * 10.0 / 12.0)
            } else {
                (h * 2.0 / 12.0, h * 5.0 / 12.0)
            };
            if name == "wedgeRectCallout" {
                // One outline, tail included, so the stroke runs round it.
                let pts: Vec<(f64, f64)> = match (vertical, dy > 0.0, dx > 0.0) {
                    (true, true, _) => {
                        vec![(0.0, 0.0), (w, 0.0), (w, h), (bx2, h), (tx, ty), (bx1, h), (0.0, h)]
                    }
                    (true, false, _) => vec![
                        (0.0, 0.0),
                        (bx1, 0.0),
                        (tx, ty),
                        (bx2, 0.0),
                        (w, 0.0),
                        (w, h),
                        (0.0, h),
                    ],
                    (false, _, true) => {
                        vec![(0.0, 0.0), (w, 0.0), (w, by1), (tx, ty), (w, by2), (w, h), (0.0, h)]
                    }
                    (false, _, false) => vec![
                        (0.0, 0.0),
                        (w, 0.0),
                        (w, h),
                        (0.0, h),
                        (0.0, by2),
                        (tx, ty),
                        (0.0, by1),
                    ],
                };
                p.poly(&pts);
                return p.done(true);
            }
            if name == "wedgeEllipseCallout" {
                p.m(0.0, h / 2.0);
                p.ellipse_arc(w / 2.0, h / 2.0, w / 2.0, h / 2.0, 180.0, 360.0);
                p.z();
            } else {
                let r = ss / 6.0;
                p.m(r, 0.0)
                    .l(w - r, 0.0)
                    .a(r, r, false, true, w, r)
                    .l(w, h - r)
                    .a(r, r, false, true, w - r, h)
                    .l(r, h)
                    .a(r, r, false, true, 0.0, h - r)
                    .l(0.0, r)
                    .a(r, r, false, true, r, 0.0)
                    .z();
            }
            // Curved bodies: the tail as a second clockwise outline (a
            // non-zero union), its base on the facing edge.
            let tail = match (vertical, dy > 0.0, dx > 0.0) {
                (true, true, _) => [(bx2, h * 0.95), (tx, ty), (bx1, h * 0.95)],
                (true, false, _) => [(bx1, h * 0.05), (tx, ty), (bx2, h * 0.05)],
                (false, _, true) => [(w * 0.95, by1), (tx, ty), (w * 0.95, by2)],
                (false, _, false) => [(w * 0.05, by2), (tx, ty), (w * 0.05, by1)],
            };
            p.poly(&tail);
            p.done(true)
        }
        _ => Geometry::Rect,
    }
}

/// A freeform (`a:custGeom`) as frame-space path commands.
fn custom_geometry(cg: &Element, w: f64, h: f64) -> Option<Geometry> {
    let guides: HashMap<&str, f64> = cg
        .find(ns::A, "gdLst")
        .map(|l| {
            l.find_all(ns::A, "gd")
                .filter_map(|g| {
                    let name = g.attr_unqualified("name")?;
                    let v =
                        g.attr_unqualified("fmla")?.strip_prefix("val ")?.trim().parse().ok()?;
                    Some((name, v))
                })
                .collect()
        })
        .unwrap_or_default();
    let value = |el: &Element, k: &str| {
        let raw = el.attr_unqualified(k).unwrap_or("0");
        parse_num(raw).or_else(|| guides.get(raw).copied()).unwrap_or(0.0)
    };
    let mut p = PathB::default();
    let mut filled = false;
    for path in cg.find(ns::A, "pathLst")?.find_all(ns::A, "path") {
        let pw = num(path, "w").filter(|v| *v > 0.0).unwrap_or(w);
        let ph = num(path, "h").filter(|v| *v > 0.0).unwrap_or(h);
        let (kx, ky) = (w / pw.max(1e-9), h / ph.max(1e-9));
        filled |= path.attr_unqualified("fill") != Some("none");
        let mut cur = (0.0, 0.0);
        let pts = |cmd: &Element| -> Vec<(f64, f64)> {
            cmd.find_all(ns::A, "pt").map(|pt| (value(pt, "x") * kx, value(pt, "y") * ky)).collect()
        };
        for cmd in path.child_elems() {
            match cmd.local.as_str() {
                "moveTo" => {
                    if let Some(&(x, y)) = pts(cmd).first() {
                        p.m(x, y);
                        cur = (x, y);
                    }
                }
                "lnTo" => {
                    if let Some(&(x, y)) = pts(cmd).first() {
                        p.l(x, y);
                        cur = (x, y);
                    }
                }
                "cubicBezTo" => {
                    if let [a, b, c] = pts(cmd)[..] {
                        p.0.push(PathCmd::CubicTo(a.0, a.1, b.0, b.1, c.0, c.1));
                        cur = c;
                    }
                }
                "quadBezTo" => {
                    if let [a, b] = pts(cmd)[..] {
                        p.0.push(PathCmd::QuadTo(a.0, a.1, b.0, b.1));
                        cur = b;
                    }
                }
                "arcTo" => {
                    let rx = value(cmd, "wR") * kx;
                    let ry = value(cmd, "hR") * ky;
                    let start = value(cmd, "stAng") / 60000.0;
                    let sweep = value(cmd, "swAng") / 60000.0;
                    let (sr, er) = (start.to_radians(), (start + sweep).to_radians());
                    let (cx, cy) = (cur.0 - rx * sr.cos(), cur.1 - ry * sr.sin());
                    p.ellipse_arc(cx, cy, rx, ry, start, sweep);
                    cur = (cx + rx * er.cos(), cy + ry * er.sin());
                }
                "close" => {
                    p.z();
                }
                _ => {}
            }
        }
    }
    (!p.0.is_empty()).then(|| p.done(filled))
}

/// Outline from a shape-properties element: `(geometry, preset name)`.
fn geometry_of(sppr: &Element, w: f64, h: f64) -> Option<(Geometry, String)> {
    if let Some(prst) = sppr.find(ns::A, "prstGeom") {
        let name = prst.attr_unqualified("prst").unwrap_or("rect");
        let adj: HashMap<String, f64> = prst
            .find(ns::A, "avLst")
            .map(|l| {
                l.find_all(ns::A, "gd")
                    .filter_map(|g| {
                        let v = g.attr_unqualified("fmla")?.strip_prefix("val ")?;
                        Some((g.attr_unqualified("name")?.to_string(), parse_num(v)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some((preset_geometry(name, &adj, w, h), name.to_string()));
    }
    let cg = sppr.find(ns::A, "custGeom")?;
    Some((custom_geometry(cg, w, h).unwrap_or(Geometry::Rect), "custom".to_string()))
}

// ---------------------------------------------------- fills, lines, effects ----

/// A fill as the source states it: unset (inherit) differs from `noFill`.
enum FillSpec {
    Unset,
    None,
    Group,
    Some(Fill),
}

fn line_dash(ln: &Element) -> Option<LineDash> {
    let v = ln.find(ns::A, "prstDash")?.attr_unqualified("val")?;
    Some(match v {
        "solid" => LineDash::Solid,
        "dot" | "sysDot" => LineDash::Dot,
        "dashDot" | "lgDashDot" | "lgDashDotDot" | "sysDashDot" | "sysDashDotDot" => {
            LineDash::DashDot
        }
        _ => LineDash::Dash,
    })
}

fn line_end(ln: &Element, which: &str) -> Option<String> {
    ln.find(ns::A, which)
        .and_then(|e| e.attr_unqualified("type"))
        .filter(|t| *t != "none")
        .map(str::to_string)
}

fn outer_shadow(effects: &Element, colors: &Colors, ph: Option<Rgba>) -> Option<Shadow> {
    let sh = effects.find(ns::A, "outerShdw")?;
    let dist = num(sh, "dist").unwrap_or(0.0);
    let dir = (num(sh, "dir").unwrap_or(0.0) / 60000.0).to_radians();
    let color = colors.of(Some(sh), ph).unwrap_or(Rgba { rgb: [0, 0, 0], alpha: 0.35 });
    Some(Shadow {
        dx: dist * dir.cos(),
        dy: dist * dir.sin(),
        blur: num(sh, "blurRad").unwrap_or(0.0),
        color,
    })
}

/// What a shape inherits for its own properties: the matching placeholder
/// shapes on the layout and master.
struct Inherited<'e> {
    layout: Option<&'e Element>,
    master: Option<&'e Element>,
}

impl<'e> Inherited<'e> {
    const NONE: Inherited<'static> = Inherited { layout: None, master: None };

    /// spPr chain, nearest first.
    fn sppr(&self) -> [Option<&'e Element>; 2] {
        [self.layout.and_then(|s| pchild(s, "spPr")), self.master.and_then(|s| pchild(s, "spPr"))]
    }
}

/// Placeholder matching: by `idx`, then by type (title kinds unify).
fn find_placeholder<'e>(tree: Option<&'e Element>, ph: &Element) -> Option<&'e Element> {
    let tree = tree?;
    let ty = ph.attr_unqualified("type").unwrap_or("body");
    let idx = ph.attr_unqualified("idx");
    let mut candidates = Vec::new();
    for el in tree.descendant_elems() {
        if el.ns.as_deref() == Some(ns::P)
            && matches!(el.local.as_str(), "sp" | "pic" | "graphicFrame")
            && let Some(other) = placeholder_of(el)
        {
            candidates.push((el, other));
        }
    }
    if let Some(idx) = idx
        && let Some((el, _)) =
            candidates.iter().find(|(_, o)| o.attr_unqualified("idx") == Some(idx))
    {
        return Some(el);
    }
    fn class(t: &str) -> &str {
        match t {
            "ctrTitle" | "title" => "title",
            "subTitle" | "obj" | "body" => "body",
            other => other,
        }
    }
    candidates
        .iter()
        .find(|(_, o)| o.attr_unqualified("type").unwrap_or("body") == ty)
        .or_else(|| {
            candidates
                .iter()
                .find(|(_, o)| class(o.attr_unqualified("type").unwrap_or("body")) == class(ty))
        })
        .map(|(el, _)| *el)
}

// --------------------------------------------------------------- tables ----

/// PowerPoint's built-in table styles are not written into the package; the
/// two that decks rely on most are restated here (from ECMA-376's preset
/// table styles): the default for new tables, and the plain grid.
const BUILTIN_TABLE_STYLES: &str = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<a:tblStyle styleId="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}" styleName="Medium Style 2 - Accent 1">
<a:wholeTbl><a:tcTxStyle><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="dk1"/></a:tcTxStyle>
<a:tcStyle><a:tcBdr>
<a:left><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:left>
<a:right><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:right>
<a:top><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top>
<a:bottom><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom>
<a:insideH><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideH>
<a:insideV><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideV>
</a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="20000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:wholeTbl>
<a:band1H><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1H>
<a:band1V><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1V>
<a:lastCol><a:tcTxStyle b="on"><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:lastCol>
<a:firstCol><a:tcTxStyle b="on"><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:firstCol>
<a:lastRow><a:tcTxStyle b="on"><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:top><a:ln w="38100"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:lastRow>
<a:firstRow><a:tcTxStyle b="on"><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:bottom><a:ln w="38100"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:firstRow>
</a:tblStyle>
<a:tblStyle styleId="{5940675A-B579-460E-94D1-54222C63F5DA}" styleName="No Style, Table Grid">
<a:wholeTbl><a:tcTxStyle><a:schemeClr val="tx1"/></a:tcTxStyle><a:tcStyle><a:tcBdr>
<a:left><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:left>
<a:right><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:right>
<a:top><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:top>
<a:bottom><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:bottom>
<a:insideH><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:insideH>
<a:insideV><a:ln w="12700"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></a:insideV>
</a:tcBdr></a:tcStyle></a:wholeTbl>
</a:tblStyle>
</a:tblStyleLst>"#;

// ------------------------------------------------------------------ driver ----

/// Per-slide resolution context.
struct Cx {
    colors: Colors,
    layout: Option<Rc<Part>>,
    master: Option<Rc<Part>>,
    default_text: LvlSet,
    title_style: LvlSet,
    body_style: LvlSet,
    other_style: LvlSet,
}

impl Cx {
    fn master_class(&self, ph_type: Option<&str>) -> &LvlSet {
        match ph_type {
            Some("title" | "ctrTitle") => &self.title_style,
            Some("dt" | "ftr" | "sldNum" | "hdr") | None => &self.other_style,
            Some(_) => &self.body_style,
        }
    }
}

/// Builds [`SlideLayout`]s for one presentation, caching shared parts.
pub(super) struct SlideLayouts<'a, 'b> {
    pkg: &'b RefCell<Package<'a>>,
    assets: &'b RefCell<AssetSink>,
    width: f64,
    height: f64,
    pres: Option<Rc<Part>>,
    parts: HashMap<String, Option<Rc<Part>>>,
    themes: HashMap<String, Rc<Theme>>,
    table_styles: Option<Rc<Part>>,
    builtin_table_styles: Option<Rc<Element>>,
    /// (part, relationship id) -> image. Master and layout pictures recur on
    /// every slide; reading them again would re-inflate their bytes.
    images: HashMap<(String, String), Option<ImageSource>>,
    /// Master/layout part -> its converted decoration, for slides that keep
    /// the master's colour map (the common case).
    decoration: HashMap<String, Rc<Vec<SlideShape>>>,
    /// Content on the current slide that had no position anywhere in its
    /// inheritance chain. A layout that would silently drop it is withheld.
    unplaced: usize,
}

impl<'a, 'b> SlideLayouts<'a, 'b> {
    pub(super) fn new(
        pkg: &'b RefCell<Package<'a>>,
        assets: &'b RefCell<AssetSink>,
        pres_part: &str,
        pres: &Element,
    ) -> Result<Self, ConvertError> {
        let size = pres.first_descendant(ns::P, "sldSz");
        let mut me = SlideLayouts {
            pkg,
            assets,
            width: size.and_then(|s| num(s, "cx")).filter(|v| *v > 0.0).unwrap_or(9144000.0),
            height: size.and_then(|s| num(s, "cy")).filter(|v| *v > 0.0).unwrap_or(6858000.0),
            pres: None,
            parts: HashMap::new(),
            themes: HashMap::new(),
            table_styles: None,
            builtin_table_styles: parse_xml(BUILTIN_TABLE_STYLES.as_bytes()).ok().map(Rc::new),
            images: HashMap::new(),
            decoration: HashMap::new(),
            unplaced: 0,
        };
        me.pres = me.load(pres_part)?;
        let styles_path = me.pres.as_ref().and_then(|p| p.view().rel_target(TABLE_STYLES_REL));
        if let Some(path) = styles_path {
            me.table_styles = me.load(&path)?;
        }
        Ok(me)
    }

    fn load(&mut self, path: &str) -> Result<Option<Rc<Part>>, ConvertError> {
        if let Some(hit) = self.parts.get(path) {
            return Ok(hit.clone());
        }
        let tree = match self.pkg.borrow_mut().optional_xml_part(path) {
            Ok(t) => t,
            Err(e) if e.is_fatal() => return Err(e),
            Err(_) => None,
        };
        let part = match tree {
            Some(tree) => {
                let rels = read_rels(&mut self.pkg.borrow_mut(), &rels_part_for(path))?;
                Some(Rc::new(Part { path: path.to_string(), tree, rels }))
            }
            None => None,
        };
        self.parts.insert(path.to_string(), part.clone());
        Ok(part)
    }

    fn theme(&mut self, master: Option<&Rc<Part>>) -> Result<Rc<Theme>, ConvertError> {
        let path = master.and_then(|m| m.view().rel_target(THEME_REL)).unwrap_or_default();
        if let Some(t) = self.themes.get(&path) {
            return Ok(t.clone());
        }
        let part = if path.is_empty() { None } else { self.load(&path)? };
        let theme = Rc::new(Theme::new(part));
        self.themes.insert(path, theme.clone());
        Ok(theme)
    }

    /// The layout of one slide. Errors other than resource limits are
    /// swallowed by the caller: a slide without a layout falls back to text.
    pub(super) fn slide(
        &mut self,
        slide_path: &str,
        tree: &Element,
        rels: &Relationships,
        anchor: &str,
    ) -> Result<Option<SlideLayout>, ConvertError> {
        let slide = PartRef { path: slide_path, tree, rels };
        let Some(slide_root) = slide.root() else { return Ok(None) };
        self.unplaced = 0;
        let layout = match slide.rel_target(LAYOUT_REL) {
            Some(p) => self.load(&p)?,
            None => None,
        };
        let master = match layout.as_ref().and_then(|l| l.view().rel_target(MASTER_REL)) {
            Some(p) => self.load(&p)?,
            None => None,
        };
        let theme = self.theme(master.as_ref())?;

        let mut map: HashMap<String, String> = master
            .as_ref()
            .and_then(|m| m.view().root())
            .and_then(|r| r.find(ns::P, "clrMap"))
            .map(|cm| cm.attrs.iter().map(|a| (a.local.clone(), a.value.clone())).collect())
            .unwrap_or_else(|| {
                [("bg1", "lt1"), ("tx1", "dk1"), ("bg2", "lt2"), ("tx2", "dk2")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            });
        for part_root in [layout.as_ref().and_then(|l| l.view().root()), Some(slide_root)] {
            if let Some(ov) = part_root
                .and_then(|r| r.find(ns::P, "clrMapOvr"))
                .and_then(|o| o.find(ns::A, "overrideClrMapping"))
            {
                map = ov.attrs.iter().map(|a| (a.local.clone(), a.value.clone())).collect();
            }
        }
        let colors = Colors { theme, map };

        let tx_styles =
            master.as_ref().and_then(|m| m.view().root()).and_then(|r| r.find(ns::P, "txStyles"));
        let default_text = level_set(
            self.pres
                .as_ref()
                .and_then(|p| p.view().root())
                .and_then(|r| r.find(ns::P, "defaultTextStyle")),
            &colors,
        );
        let cx = Cx {
            title_style: level_set(tx_styles.and_then(|t| t.find(ns::P, "titleStyle")), &colors),
            body_style: level_set(tx_styles.and_then(|t| t.find(ns::P, "bodyStyle")), &colors),
            other_style: level_set(tx_styles.and_then(|t| t.find(ns::P, "otherStyle")), &colors),
            default_text,
            colors,
            layout: layout.clone(),
            master: master.clone(),
        };

        let background = {
            let mut bg = None;
            for part in
                [Some(slide), layout.as_ref().map(|l| l.view()), master.as_ref().map(|m| m.view())]
                    .into_iter()
                    .flatten()
            {
                if let Some(el) = part
                    .root()
                    .and_then(|r| r.find(ns::P, "cSld"))
                    .and_then(|c| c.find(ns::P, "bg"))
                {
                    bg = self.background(&cx, part, el)?;
                    break;
                }
            }
            bg
        };

        let mut shapes = Vec::new();
        let shows =
            |root: Option<&Element>| root.and_then(|r| flag(r, "showMasterSp")) != Some(false);
        let slide_shows = shows(Some(slide_root));
        // Decoration depends only on its part and the colour map; a slide
        // that overrides the map gets its own conversion.
        let cacheable = slide_root
            .find(ns::P, "clrMapOvr")
            .and_then(|o| o.find(ns::A, "overrideClrMapping"))
            .is_none();
        let decor = [
            (slide_shows && shows(layout.as_ref().and_then(|l| l.view().root())))
                .then_some(master.as_ref())
                .flatten(),
            slide_shows.then_some(layout.as_ref()).flatten(),
        ];
        for part in decor.into_iter().flatten() {
            if cacheable && let Some(hit) = self.decoration.get(&part.path) {
                shapes.extend(hit.iter().cloned());
                continue;
            }
            let mut converted = Vec::new();
            if let Some(tree) = part.view().sp_tree() {
                self.tree(&cx, part.view(), tree, Tf::IDENTITY, None, false, &mut converted)?;
            }
            shapes.extend(converted.iter().cloned());
            if cacheable {
                self.decoration.insert(part.path.clone(), Rc::new(converted));
            }
        }
        if let Some(tree) = slide.sp_tree() {
            self.tree(&cx, slide, tree, Tf::IDENTITY, None, true, &mut shapes)?;
        }

        if self.unplaced > 0 {
            log::warn!(
                "slide {slide_path}: {} item(s) have no position; no fixed layout",
                self.unplaced
            );
            return Ok(None);
        }
        Ok(Some(SlideLayout {
            anchor: anchor.to_string(),
            width: self.width,
            height: self.height,
            background,
            shapes,
        }))
    }

    fn background(
        &mut self,
        cx: &Cx,
        part: PartRef,
        bg: &Element,
    ) -> Result<Option<Fill>, ConvertError> {
        if let Some(pr) = bg.find(ns::P, "bgPr") {
            return Ok(match self.fill_spec(cx, part, pr, None)? {
                FillSpec::Some(f) => Some(f),
                _ => None,
            });
        }
        if let Some(r) = bg.find(ns::P, "bgRef") {
            let ph = cx.colors.of(Some(r), None);
            let idx = num(r, "idx").unwrap_or(0.0) as usize;
            let entry = if idx >= 1001 {
                cx.colors.theme.style_entry("bgFillStyleLst", idx - 1000)
            } else {
                cx.colors.theme.style_entry("fillStyleLst", idx)
            };
            if let Some(entry) = entry
                && let FillSpec::Some(f) = self.fill_elem(cx, part, entry, ph)?
            {
                return Ok(Some(f));
            }
            return Ok(ph.map(Fill::Solid));
        }
        Ok(None)
    }

    /// The first fill-bearing child of `parent`.
    fn fill_spec(
        &mut self,
        cx: &Cx,
        part: PartRef,
        parent: &Element,
        ph: Option<Rgba>,
    ) -> Result<FillSpec, ConvertError> {
        for child in parent.child_elems() {
            match self.fill_elem(cx, part, child, ph)? {
                FillSpec::Unset => continue,
                other => return Ok(other),
            }
        }
        Ok(FillSpec::Unset)
    }

    fn fill_elem(
        &mut self,
        cx: &Cx,
        part: PartRef,
        el: &Element,
        ph: Option<Rgba>,
    ) -> Result<FillSpec, ConvertError> {
        if el.ns.as_deref() != Some(ns::A) {
            return Ok(FillSpec::Unset);
        }
        Ok(match el.local.as_str() {
            "noFill" => FillSpec::None,
            "grpFill" => FillSpec::Group,
            "solidFill" => match cx.colors.of(Some(el), ph) {
                Some(c) => FillSpec::Some(Fill::Solid(c)),
                None => FillSpec::None,
            },
            "pattFill" => match cx.colors.of(el.find(ns::A, "fgClr"), ph) {
                Some(c) => FillSpec::Some(Fill::Solid(c)),
                None => FillSpec::None,
            },
            "gradFill" => {
                let stops: Vec<(f64, Rgba)> = el
                    .find(ns::A, "gsLst")
                    .map(|l| {
                        l.find_all(ns::A, "gs")
                            .filter_map(|gs| {
                                let pos =
                                    (num(gs, "pos").unwrap_or(0.0) / 100000.0).clamp(0.0, 1.0);
                                Some((pos, cx.colors.of(Some(gs), ph)?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if stops.is_empty() {
                    FillSpec::Unset
                } else {
                    let angle =
                        el.find(ns::A, "lin").and_then(|l| num(l, "ang")).unwrap_or(5400000.0)
                            / 60000.0;
                    let radial = el.find(ns::A, "path").is_some();
                    FillSpec::Some(Fill::Gradient { angle, radial, stops })
                }
            }
            "blipFill" => match self.blip(part, el)? {
                Some(src) => FillSpec::Some(Fill::Image(src)),
                None => FillSpec::Unset,
            },
            _ => FillSpec::Unset,
        })
    }

    fn blip(
        &mut self,
        part: PartRef,
        blip_fill: &Element,
    ) -> Result<Option<ImageSource>, ConvertError> {
        let Some(blip) = blip_fill.find(ns::A, "blip") else { return Ok(None) };
        let rid =
            blip.attr_qualified(ns::R, "embed").or_else(|| blip.attr_qualified(ns::R, "link"));
        let Some(rid) = rid else { return Ok(None) };
        let key = (part.path.to_string(), rid.to_string());
        if let Some(hit) = self.images.get(&key) {
            return Ok(hit.clone());
        }
        let source = rel_image_source(self.pkg, part.rels, part.path, self.assets, rid)?;
        self.images.insert(key, source.clone());
        Ok(source)
    }

    /// Fill through the inheritance chain: own spPr, layout/master
    /// placeholder spPr, then the style reference.
    fn resolve_fill(
        &mut self,
        cx: &Cx,
        part: PartRef,
        sppr: Option<&Element>,
        inherited: &Inherited,
        style: Option<&Element>,
        group_fill: Option<&Fill>,
    ) -> Result<Option<Fill>, ConvertError> {
        let mut chain = vec![sppr];
        chain.extend(inherited.sppr());
        for (i, el) in chain.into_iter().enumerate() {
            let Some(el) = el else { continue };
            // Inherited spPr images resolve against their own part; the
            // shape's own part covers the common case.
            let spec = if i == 0 {
                self.fill_spec(cx, part, el, None)?
            } else {
                self.fill_spec(cx, part, el, None).unwrap_or(FillSpec::Unset)
            };
            match spec {
                FillSpec::Unset => continue,
                FillSpec::None => return Ok(None),
                FillSpec::Group => return Ok(group_fill.cloned()),
                FillSpec::Some(f) => return Ok(Some(f)),
            }
        }
        if let Some(r) = style.and_then(|s| s.find(ns::A, "fillRef")) {
            let ph = cx.colors.of(Some(r), None);
            let idx = num(r, "idx").unwrap_or(0.0) as usize;
            let entry = if idx >= 1001 {
                cx.colors.theme.style_entry("bgFillStyleLst", idx - 1000)
            } else {
                cx.colors.theme.style_entry("fillStyleLst", idx)
            };
            if let Some(entry) = entry {
                return Ok(match self.fill_elem(cx, part, entry, ph)? {
                    FillSpec::Some(f) => Some(f),
                    _ => None,
                });
            }
            if idx > 0 {
                return Ok(ph.map(Fill::Solid));
            }
        }
        Ok(None)
    }

    fn resolve_line(
        &self,
        cx: &Cx,
        sppr: Option<&Element>,
        inherited: &Inherited,
        style: Option<&Element>,
    ) -> Option<Line> {
        let mut lns: Vec<&Element> = Vec::new();
        for el in [sppr].into_iter().chain(inherited.sppr()).flatten() {
            if let Some(ln) = el.find(ns::A, "ln") {
                lns.push(ln);
            }
        }
        let style_ref = style.and_then(|s| s.find(ns::A, "lnRef"));
        let ph = style_ref.and_then(|r| cx.colors.of(Some(r), None));
        let theme_ln = style_ref
            .and_then(|r| num(r, "idx"))
            .filter(|i| *i > 0.0)
            .and_then(|i| cx.colors.theme.style_entry("lnStyleLst", i as usize));
        // Colour: nearest explicit fill wins; noFill anywhere nearer kills it.
        let mut color = None;
        let mut decided = false;
        for ln in lns.iter().copied().chain(theme_ln) {
            for c in ln.child_elems() {
                match c.local.as_str() {
                    "noFill" => {
                        decided = true;
                    }
                    "solidFill" => {
                        color = cx.colors.of(Some(c), ph);
                        decided = true;
                    }
                    "gradFill" => {
                        color = c
                            .find(ns::A, "gsLst")
                            .and_then(|l| l.find(ns::A, "gs"))
                            .and_then(|gs| cx.colors.of(Some(gs), ph));
                        decided = true;
                    }
                    _ => continue,
                }
                break;
            }
            if decided {
                break;
            }
        }
        if !decided && theme_ln.is_none() {
            color = ph.filter(|_| style_ref.is_some_and(|r| num(r, "idx").unwrap_or(0.0) > 0.0));
        }
        let color = color?;
        let all = || lns.iter().copied().chain(theme_ln);
        let width = all().find_map(|l| num(l, "w")).unwrap_or(9525.0);
        Some(Line {
            width,
            color,
            dash: all().find_map(line_dash).unwrap_or(LineDash::Solid),
            head_end: all().find_map(|l| line_end(l, "headEnd")),
            tail_end: all().find_map(|l| line_end(l, "tailEnd")),
        })
    }

    fn resolve_shadow(
        &self,
        cx: &Cx,
        sppr: Option<&Element>,
        inherited: &Inherited,
        style: Option<&Element>,
    ) -> Option<Shadow> {
        for el in [sppr].into_iter().chain(inherited.sppr()).flatten() {
            if let Some(effects) = el.find(ns::A, "effectLst") {
                return outer_shadow(effects, &cx.colors, None);
            }
        }
        let r = style?.find(ns::A, "effectRef")?;
        let idx = num(r, "idx").filter(|i| *i > 0.0)? as usize;
        let ph = cx.colors.of(Some(r), None);
        let entry = cx.colors.theme.style_entry("effectStyleLst", idx)?;
        outer_shadow(entry.find(ns::A, "effectLst")?, &cx.colors, ph)
    }

    #[allow(clippy::too_many_arguments)]
    fn tree(
        &mut self,
        cx: &Cx,
        part: PartRef,
        tree: &Element,
        tf: Tf,
        group_fill: Option<&Fill>,
        is_slide: bool,
        out: &mut Vec<SlideShape>,
    ) -> Result<(), ConvertError> {
        for child in tree.child_elems() {
            if child.is(ns::MC, "AlternateContent") {
                if let Some(branch) = crate::shared::mc::alternate_branch(child, SUPPORTED_NS) {
                    self.tree(cx, part, branch, tf, group_fill, is_slide, out)?;
                }
                continue;
            }
            if !matches!(child.ns.as_deref(), Some(ns::P) | Some(DSP)) || is_hidden(child) {
                continue;
            }
            let result = match child.local.as_str() {
                "sp" | "cxnSp" => self.shape(cx, part, child, tf, group_fill, is_slide, out),
                "pic" => self.picture(cx, part, child, tf, None, is_slide, out),
                "grpSp" => {
                    let gpr = pchild(child, "grpSpPr");
                    let inner =
                        gpr.and_then(|g| g.find(ns::A, "xfrm")).map(|x| tf.child(x)).unwrap_or(tf);
                    let fill = match gpr {
                        Some(g) => match self.fill_spec(cx, part, g, None)? {
                            FillSpec::Some(f) => Some(f),
                            FillSpec::Group => group_fill.cloned(),
                            _ => None,
                        },
                        None => None,
                    };
                    self.tree(cx, part, child, inner, fill.as_ref().or(group_fill), is_slide, out)
                }
                "graphicFrame" => self.frame(cx, part, child, tf, is_slide, out),
                _ => Ok(()),
            };
            match result {
                Ok(()) => {}
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => log::warn!("slide layout: skipping a shape in {}: {e}", part.path),
            }
        }
        Ok(())
    }

    /// Placeholder inheritance for a slide shape (layout and master shapes
    /// never inherit: their placeholders are not drawn).
    fn inherited<'c>(
        &self,
        cx: &'c Cx,
        el: &Element,
        is_slide: bool,
    ) -> (Option<String>, Inherited<'c>) {
        let Some(ph) = placeholder_of(el) else { return (None, Inherited::NONE) };
        let ty = Some(ph.attr_unqualified("type").unwrap_or("body").to_string());
        if !is_slide {
            return (ty, Inherited::NONE);
        }
        let layout = find_placeholder(cx.layout.as_ref().and_then(|l| l.view().sp_tree()), ph);
        // The master match follows the layout placeholder's identity when
        // there is one (the slide's idx is the layout's, not the master's).
        let master_key = layout.and_then(placeholder_of).unwrap_or(ph);
        let master =
            find_placeholder(cx.master.as_ref().and_then(|m| m.view().sp_tree()), master_key)
                .or_else(|| {
                    find_placeholder(cx.master.as_ref().and_then(|m| m.view().sp_tree()), ph)
                });
        (ty, Inherited { layout, master })
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(
        &mut self,
        cx: &Cx,
        part: PartRef,
        el: &Element,
        tf: Tf,
        group_fill: Option<&Fill>,
        is_slide: bool,
        out: &mut Vec<SlideShape>,
    ) -> Result<(), ConvertError> {
        let ph = placeholder_of(el);
        if ph.is_some() && !is_slide && el.ns.as_deref() == Some(ns::P) {
            return Ok(());
        }
        let (ph_type, inherited) = self.inherited(cx, el, is_slide);
        let sppr = pchild(el, "spPr");
        let Some(frame) = [sppr]
            .into_iter()
            .chain(inherited.sppr())
            .flatten()
            .find_map(|s| read_xfrm(s.find(ns::A, "xfrm")))
        else {
            if pchild(el, "txBody").is_some_and(|t| !t.text().trim().is_empty()) {
                self.unplaced += 1;
            }
            return Ok(());
        };
        let frame = tf.frame(frame);
        let style = pchild(el, "style");
        let (geometry, preset) = [sppr]
            .into_iter()
            .chain(inherited.sppr())
            .flatten()
            .find_map(|s| geometry_of(s, frame.width, frame.height))
            .unwrap_or((Geometry::Rect, "rect".to_string()));
        let fill = self.resolve_fill(cx, part, sppr, &inherited, style, group_fill)?;
        let line = self.resolve_line(cx, sppr, &inherited, style);
        let shadow = self.resolve_shadow(cx, sppr, &inherited, style);

        let mut text = None;
        if let Some(tx) = pchild(el, "txBody") {
            let mut chain: Vec<&LvlSet> =
                vec![&cx.default_text, cx.master_class(ph_type.as_deref())];
            let mas_lst = inherited
                .master
                .and_then(|s| pchild(s, "txBody"))
                .map(|t| level_set(t.find(ns::A, "lstStyle"), &cx.colors));
            let lay_lst = inherited
                .layout
                .and_then(|s| pchild(s, "txBody"))
                .map(|t| level_set(t.find(ns::A, "lstStyle"), &cx.colors));
            chain.extend(mas_lst.iter());
            chain.extend(lay_lst.iter());
            let over = RunProps {
                color: style
                    .and_then(|s| s.find(ns::A, "fontRef"))
                    .and_then(|r| cx.colors.of(Some(r), None)),
                ..RunProps::default()
            };
            let body = tx.find(ns::A, "bodyPr");
            let body_chain = [
                inherited
                    .master
                    .and_then(|s| pchild(s, "txBody"))
                    .and_then(|t| t.find(ns::A, "bodyPr")),
                inherited
                    .layout
                    .and_then(|s| pchild(s, "txBody"))
                    .and_then(|t| t.find(ns::A, "bodyPr")),
                body,
            ];
            let (insets, anchor, wrap) = body_props(&body_chain);
            let autofit = body.and_then(|b| b.find(ns::A, "normAutofit"));
            let font_scale = autofit
                .and_then(|a| num(a, "fontScale"))
                .map(|v| (v / 100000.0).clamp(0.01, 1.0))
                .unwrap_or(1.0);
            let line_reduction = autofit
                .and_then(|a| num(a, "lnSpcReduction"))
                .map(|v| (v / 100000.0).clamp(0.0, 0.9))
                .unwrap_or(0.0);
            let paras = paragraphs(
                tx,
                &TextInherit { chain, over },
                &cx.colors,
                font_scale,
                line_reduction,
            );
            if paras.iter().any(|p| p.runs.iter().any(|r| !r.text.trim().is_empty())) {
                text = Some(TextFrame { insets, anchor, wrap, paragraphs: paras });
            }
        }

        // SmartArt drawings keep the text box separately (dsp:txXfrm).
        let text_frame = pchild(el, "txXfrm").and_then(|x| read_xfrm(Some(x))).map(|f| tf.frame(f));
        let (own_text, split_text) = match (text, text_frame) {
            (Some(t), Some(f)) => (None, Some((t, f))),
            (t, _) => (t, None),
        };
        if fill.is_some() || line.is_some() || own_text.is_some() {
            out.push(SlideShape {
                frame,
                geometry,
                preset,
                fill,
                line,
                shadow,
                content: own_text.map(ShapeContent::Text).unwrap_or(ShapeContent::None),
            });
        }
        if let Some((t, f)) = split_text {
            out.push(SlideShape {
                frame: Frame { rotation: f.rotation, ..f },
                geometry: Geometry::Rect,
                preset: "rect".to_string(),
                fill: None,
                line: None,
                shadow: None,
                content: ShapeContent::Text(t),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn picture(
        &mut self,
        cx: &Cx,
        part: PartRef,
        el: &Element,
        tf: Tf,
        frame_override: Option<Frame>,
        is_slide: bool,
        out: &mut Vec<SlideShape>,
    ) -> Result<(), ConvertError> {
        if placeholder_of(el).is_some() && !is_slide {
            return Ok(());
        }
        let (_, inherited) = self.inherited(cx, el, is_slide);
        let sppr = pchild(el, "spPr");
        let frame = match frame_override {
            Some(f) => f,
            None => {
                let Some(f) = [sppr]
                    .into_iter()
                    .chain(inherited.sppr())
                    .flatten()
                    .find_map(|s| read_xfrm(s.find(ns::A, "xfrm")))
                else {
                    self.unplaced += 1;
                    return Ok(());
                };
                tf.frame(f)
            }
        };
        let blip_fill = pchild(el, "blipFill");
        let source = match blip_fill {
            Some(bf) => self.blip(part, bf)?,
            None => None,
        };
        let crop = blip_fill
            .and_then(|bf| bf.find(ns::A, "srcRect"))
            .map(|r| {
                let f = |k: &str| num(r, k).unwrap_or(0.0) / 100000.0;
                Crop { left: f("l"), top: f("t"), right: f("r"), bottom: f("b") }
            })
            .unwrap_or_default();
        let style = pchild(el, "style");
        let (geometry, preset) = sppr
            .and_then(|s| geometry_of(s, frame.width, frame.height))
            .unwrap_or((Geometry::Rect, "rect".to_string()));
        out.push(SlideShape {
            frame,
            geometry,
            preset,
            fill: None,
            line: self.resolve_line(cx, sppr, &inherited, style),
            shadow: self.resolve_shadow(cx, sppr, &inherited, style),
            content: ShapeContent::Image {
                source: source.unwrap_or(ImageSource::Unavailable),
                crop,
            },
        });
        Ok(())
    }

    fn frame(
        &mut self,
        cx: &Cx,
        part: PartRef,
        el: &Element,
        tf: Tf,
        is_slide: bool,
        out: &mut Vec<SlideShape>,
    ) -> Result<(), ConvertError> {
        let (_, inherited) = self.inherited(cx, el, is_slide);
        let own = read_xfrm(pchild(el, "xfrm"));
        let inherited_xfrm =
            [inherited.layout, inherited.master].into_iter().flatten().find_map(|s| {
                read_xfrm(pchild(s, "xfrm"))
                    .or_else(|| read_xfrm(pchild(s, "spPr").and_then(|p| p.find(ns::A, "xfrm"))))
            });
        let Some(frame) = own.or(inherited_xfrm) else {
            self.unplaced += 1;
            return Ok(());
        };
        let frame = tf.frame(frame);
        let Some(data) = el.find(ns::A, "graphic").and_then(|g| g.find(ns::A, "graphicData"))
        else {
            return Ok(());
        };
        let plain = |content: ShapeContent| SlideShape {
            frame,
            geometry: Geometry::Rect,
            preset: "rect".to_string(),
            fill: None,
            line: None,
            shadow: None,
            content,
        };
        if let Some(tbl) = data.find(ns::A, "tbl") {
            let table = self.table(cx, part, tbl)?;
            out.push(plain(ShapeContent::Table(table)));
            return Ok(());
        }
        if let Some(rid) =
            data.first_descendant(ns::CHART, "chart").and_then(|c| c.attr_qualified(ns::R, "id"))
            && let Some(path) = part.rel_path(rid)
            && let Some(chart_part) = self.load(&path)?
        {
            let blocks = crate::shared::drawingml::chart_blocks(&chart_part.tree);
            if let Some(chart) = blocks.into_iter().find_map(|b| match b {
                Block::Chart(c) => Some(c),
                _ => None,
            }) {
                out.push(plain(ShapeContent::Chart(chart)));
            }
            return Ok(());
        }
        if let Some(rel_ids) = data.first_descendant(ns::DGM, "relIds") {
            return self.diagram(cx, part, rel_ids, frame, out);
        }
        // OLE objects and other embeds: their preview picture.
        if let Some(pic) = data.first_descendant(ns::P, "pic") {
            return self.picture(cx, part, pic, tf, Some(frame), true, out);
        }
        Ok(())
    }

    /// SmartArt: draw PowerPoint's saved drawing when present, else the
    /// diagram's text in the frame.
    fn diagram(
        &mut self,
        cx: &Cx,
        part: PartRef,
        rel_ids: &Element,
        frame: Frame,
        out: &mut Vec<SlideShape>,
    ) -> Result<(), ConvertError> {
        let data = match rel_ids.attr_qualified(ns::R, "dm").and_then(|rid| part.rel_path(rid)) {
            Some(p) => self.load(&p)?,
            None => None,
        };
        let drawing_path = data.as_ref().and_then(|d| {
            d.tree
                .descendants_any("dataModelExt")
                .find_map(|e| e.attr_unqualified("relId"))
                .and_then(|rid| part.rel_path(rid))
        });
        if let Some(path) = drawing_path
            && let Some(drawing) = self.load(&path)?
            && let Some(tree) = drawing.view().root().and_then(|r| pchild(r, "spTree"))
        {
            let tf = Tf { ox: frame.x, oy: frame.y, sx: 1.0, sy: 1.0 };
            return self.tree(cx, drawing.view(), tree, tf, None, false, out);
        }
        let Some(data) = data else { return Ok(()) };
        let mut paras = Vec::new();
        for block in crate::shared::drawingml::diagram_blocks(&data.tree) {
            let mut lines = Vec::new();
            block_text(&block, &mut lines);
            for text in lines {
                paras.push(SlideParagraph {
                    align: SlideAlign::Center,
                    margin_left: 0.0,
                    indent: 0.0,
                    line_spacing: None,
                    space_before: Some(Spacing::Points(600.0)),
                    space_after: None,
                    bullet: None,
                    runs: vec![SlideRun {
                        text,
                        size: 1400.0,
                        font: cx.colors.theme.minor.clone(),
                        bold: false,
                        italic: false,
                        underline: false,
                        strike: false,
                        color: cx.colors.scheme("tx1").unwrap_or(BLACK),
                        letter_spacing: 0.0,
                        baseline: 0.0,
                        all_caps: false,
                    }],
                    end_size: 1400.0,
                });
            }
        }
        if !paras.is_empty() {
            out.push(SlideShape {
                frame,
                geometry: Geometry::Rect,
                preset: "rect".to_string(),
                fill: None,
                line: None,
                shadow: None,
                content: ShapeContent::Text(TextFrame {
                    insets: [91440.0, 45720.0, 91440.0, 45720.0],
                    anchor: TextAnchor::Middle,
                    wrap: true,
                    paragraphs: paras,
                }),
            });
        }
        Ok(())
    }

    fn table(&mut self, cx: &Cx, part: PartRef, tbl: &Element) -> Result<SlideTable, ConvertError> {
        let tbl_pr = tbl.find(ns::A, "tblPr");
        let opt = |k: &str| tbl_pr.and_then(|p| flag(p, k)).unwrap_or(false);
        let (first_row, last_row, first_col, last_col, band_row, band_col) = (
            opt("firstRow"),
            opt("lastRow"),
            opt("firstCol"),
            opt("lastCol"),
            opt("bandRow"),
            opt("bandCol"),
        );
        let (custom, builtin) = (self.table_styles.clone(), self.builtin_table_styles.clone());
        let style = table_style(custom.as_ref().map(|p| &p.tree), builtin.as_deref(), tbl_pr);
        let columns: Vec<f64> = tbl
            .find(ns::A, "tblGrid")
            .map(|g| g.find_all(ns::A, "gridCol").map(|c| num(c, "w").unwrap_or(0.0)).collect())
            .unwrap_or_default();
        let rows_el: Vec<&Element> = tbl.find_all(ns::A, "tr").collect();
        let (n_rows, n_cols) = (rows_el.len(), columns.len());
        let mut rows = Vec::with_capacity(n_rows);
        for (r, tr) in rows_el.iter().enumerate() {
            let mut cells = Vec::new();
            for (c, tc) in tr.find_all(ns::A, "tc").enumerate() {
                if flag(tc, "hMerge") == Some(true) || flag(tc, "vMerge") == Some(true) {
                    continue;
                }
                let col_span =
                    num(tc, "gridSpan").map(|v| v.clamp(1.0, 1000.0) as usize).unwrap_or(1);
                let row_span =
                    num(tc, "rowSpan").map(|v| v.clamp(1.0, 1000.0) as usize).unwrap_or(1);
                let last_c = c + col_span >= n_cols;
                let last_r = r + row_span >= n_rows;

                // Style parts in increasing precedence.
                let mut parts: Vec<&str> = vec!["wholeTbl"];
                let data_r = r.saturating_sub(usize::from(first_row));
                if band_row && !(first_row && r == 0) && !(last_row && last_r) {
                    parts.push(if data_r % 2 == 0 { "band1H" } else { "band2H" });
                }
                let data_c = c.saturating_sub(usize::from(first_col));
                if band_col && !(first_col && c == 0) && !(last_col && last_c) {
                    parts.push(if data_c % 2 == 0 { "band1V" } else { "band2V" });
                }
                if first_col && c == 0 {
                    parts.push("firstCol");
                }
                if last_col && last_c {
                    parts.push("lastCol");
                }
                if first_row && r == 0 {
                    parts.push("firstRow");
                }
                if last_row && last_r {
                    parts.push("lastRow");
                }

                let mut fill: Option<Fill> = None;
                let mut text_over = RunProps::default();
                let mut borders: [Option<Line>; 4] = [None, None, None, None];
                if let Some(style) = style {
                    for name in &parts {
                        let Some(sp) = style.find(ns::A, name) else { continue };
                        if let Some(ts) = sp.find(ns::A, "tcStyle") {
                            if let Some(f) = ts.find(ns::A, "fill") {
                                match self.fill_spec(cx, part, f, None)? {
                                    FillSpec::Some(x) => fill = Some(x),
                                    FillSpec::None => fill = None,
                                    _ => {}
                                }
                            } else if let Some(fr) = ts.find(ns::A, "fillRef") {
                                let ph = cx.colors.of(Some(fr), None);
                                fill = ph.map(Fill::Solid).or(fill);
                            }
                            if let Some(bdr) = ts.find(ns::A, "tcBdr") {
                                let edge = |outer: &str, inner: &str, is_outer: bool| {
                                    bdr.find(ns::A, if is_outer { outer } else { inner })
                                };
                                let sides = [
                                    edge("left", "insideV", c == 0),
                                    edge("top", "insideH", r == 0),
                                    edge("right", "insideV", last_c),
                                    edge("bottom", "insideH", last_r),
                                ];
                                for (slot, side) in borders.iter_mut().zip(sides) {
                                    if let Some(side) = side {
                                        *slot = side
                                            .find(ns::A, "ln")
                                            .and_then(|ln| table_line(ln, &cx.colors))
                                            .or_else(|| {
                                                side.find(ns::A, "lnRef").and_then(|lr| {
                                                    Some(Line {
                                                        width: 12700.0,
                                                        color: cx.colors.of(Some(lr), None)?,
                                                        dash: LineDash::Solid,
                                                        head_end: None,
                                                        tail_end: None,
                                                    })
                                                })
                                            });
                                    }
                                }
                            }
                        }
                        if let Some(tts) = sp.find(ns::A, "tcTxStyle") {
                            if let Some(b) = flag(tts, "b") {
                                text_over.bold = Some(b);
                            }
                            if let Some(i) = flag(tts, "i") {
                                text_over.italic = Some(i);
                            }
                            let direct = tts
                                .child_elems()
                                .filter(|e| e.local != "fontRef" && e.local != "font")
                                .find_map(|e| cx.colors.resolve(e, None));
                            let via_ref = cx.colors.of(tts.find(ns::A, "fontRef"), None);
                            if let Some(col) = direct.or(via_ref) {
                                text_over.color = Some(col);
                            }
                        }
                    }
                }

                let tc_pr = tc.find(ns::A, "tcPr");
                if let Some(pr) = tc_pr {
                    match self.fill_spec(cx, part, pr, None)? {
                        FillSpec::Some(f) => fill = Some(f),
                        FillSpec::None => fill = None,
                        _ => {}
                    }
                    for (slot, name) in borders.iter_mut().zip(["lnL", "lnT", "lnR", "lnB"]) {
                        if let Some(ln) = pr.find(ns::A, name) {
                            *slot = table_line(ln, &cx.colors);
                        }
                    }
                }
                let margin = |k: &str, d: f64| tc_pr.and_then(|p| num(p, k)).unwrap_or(d);
                let anchor = match tc_pr.and_then(|p| p.attr_unqualified("anchor")) {
                    Some("ctr") => TextAnchor::Middle,
                    Some("b") => TextAnchor::Bottom,
                    _ => TextAnchor::Top,
                };
                let paras = match tc.find(ns::A, "txBody") {
                    Some(tx) => paragraphs(
                        tx,
                        &TextInherit {
                            chain: vec![&cx.default_text, &cx.other_style],
                            over: text_over,
                        },
                        &cx.colors,
                        1.0,
                        0.0,
                    ),
                    None => Vec::new(),
                };
                cells.push(SlideTableCell {
                    col: c,
                    col_span,
                    row_span,
                    fill,
                    borders,
                    insets: [
                        margin("marL", 91440.0),
                        margin("marT", 45720.0),
                        margin("marR", 91440.0),
                        margin("marB", 45720.0),
                    ],
                    anchor,
                    paragraphs: paras,
                });
            }
            rows.push(SlideTableRow { height: num(tr, "h").unwrap_or(0.0), cells });
        }
        Ok(SlideTable { columns, rows })
    }
}

/// The table's style: the package's own definitions first, then the
/// built-in ones PowerPoint does not write out.
fn table_style<'e>(
    custom: Option<&'e Element>,
    builtin: Option<&'e Element>,
    tbl_pr: Option<&Element>,
) -> Option<&'e Element> {
    let id = tbl_pr?.find(ns::A, "tableStyleId")?.text();
    let id = id.trim();
    let find = |root: &'e Element| {
        root.descendants(ns::A, "tblStyle").find(|s| s.attr_unqualified("styleId") == Some(id))
    };
    custom.and_then(find).or_else(|| builtin.and_then(find))
}

/// A table border line; `noFill` (or no colour) means no border.
fn table_line(ln: &Element, colors: &Colors) -> Option<Line> {
    if ln.find(ns::A, "noFill").is_some() {
        return None;
    }
    let color = colors.of(ln.find(ns::A, "solidFill"), None)?;
    Some(Line {
        width: num(ln, "w").unwrap_or(12700.0),
        color,
        dash: line_dash(ln).unwrap_or(LineDash::Solid),
        head_end: None,
        tail_end: None,
    })
}

fn block_text(block: &Block, out: &mut Vec<String>) {
    match block {
        Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
            let text = crate::model::inlines_to_plain_text(inlines);
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
        Block::List(list) => {
            for item in &list.items {
                for b in &item.blocks {
                    block_text(b, out);
                }
            }
        }
        Block::BlockQuote(inner) => inner.iter().for_each(|b| block_text(b, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> Colors {
        Colors {
            theme: Rc::new(Theme::new(None)),
            map: [("tx1", "dk1"), ("bg1", "lt1")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// Parse a DrawingML snippet, declaring the `a:` namespace on its root.
    fn el(xml: &str) -> Element {
        let end = xml.find([' ', '>', '/']).unwrap();
        let xml = format!(r#"{} xmlns:a="{}"{}"#, &xml[..end], ns::A, &xml[end..]);
        parse_xml(xml.as_bytes()).unwrap()
    }

    fn first(root: &Element) -> &Element {
        root.child_elems().next().unwrap()
    }

    #[test]
    fn scheme_colour_modifiers_follow_powerpoint() {
        let c = colors();
        // accent1 4472C4 at lumMod 75% (PowerPoint's "Darker 25%") is 2F5597.
        let root = el(r#"<a:schemeClr val="accent1"><a:lumMod val="75000"/></a:schemeClr>"#);
        let got = c.resolve(first(&root), None).unwrap();
        assert_eq!(got.rgb, [0x2F, 0x55, 0x97]);
        // tx1 maps through clrMap to dk1.
        let root = el(r#"<a:schemeClr val="tx1"/>"#);
        assert_eq!(c.resolve(first(&root), None).unwrap().rgb, [0, 0, 0]);
        // alpha carries through.
        let root = el(r#"<a:srgbClr val="FF0000"><a:alpha val="40000"/></a:srgbClr>"#);
        let got = c.resolve(first(&root), None).unwrap();
        assert_eq!((got.rgb, (got.alpha * 100.0).round()), ([255, 0, 0], 40.0));
    }

    #[test]
    fn freeform_scales_path_space_to_the_frame() {
        let root = el(
            r#"<a:custGeom><a:pathLst><a:path w="100" h="50"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="100" y="50"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom>"#,
        );
        let g = custom_geometry(first(&root), 1000.0, 1000.0).unwrap();
        assert_eq!(
            g,
            Geometry::Path {
                commands: vec![
                    PathCmd::MoveTo(0.0, 0.0),
                    PathCmd::LineTo(1000.0, 1000.0),
                    PathCmd::Close
                ],
                filled: true
            }
        );
    }

    #[test]
    fn presets_resolve_to_drawable_outlines() {
        let none = HashMap::new();
        assert_eq!(
            preset_geometry("roundRect", &none, 100.0, 50.0),
            Geometry::RoundRect { radius: 0.16667 }
        );
        assert_eq!(preset_geometry("ellipse", &none, 100.0, 50.0), Geometry::Ellipse);
        assert!(matches!(
            preset_geometry("chevron", &none, 100.0, 50.0),
            Geometry::Path { filled: true, .. }
        ));
        assert_eq!(preset_geometry("cloudCallout", &none, 100.0, 50.0), Geometry::Rect);
    }

    #[test]
    fn symbol_font_bullets_map_to_unicode() {
        assert_eq!(bullet_glyph("§", Some("Wingdings")), ("■".to_string(), true));
        assert_eq!(bullet_glyph("•", Some("Arial")), ("•".to_string(), false));
    }

    #[test]
    fn run_text_keeps_soft_breaks() {
        assert_eq!(run_text("a\u{b}b\r\nc\u{7}"), "a\nb\nc");
    }
}
