//! WordprocessingML style table.
//!
//! Bold/italic/strike are *toggle properties* (ECMA-376 §17.7.3): within the
//! style hierarchy a `true` specification toggles the inherited value and a
//! `false` specification leaves it unchanged, so the style layers contribute
//! a true-count *parity* XORed over the `docDefaults` base. Direct run
//! formatting is absolute on/off.

use crate::error::ConvertError;
use crate::model::{Align, FontId, ParaProps, Style};
use crate::package::xml::{Element, ns};
use crate::shared::chain::StyleChains;
use crate::shared::delta::StyleDelta;
use std::cell::RefCell;

/// Per-property parity of `true` toggle specifications in a style chain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

impl Toggles {
    pub fn xor(self, other: Toggles) -> Toggles {
        Toggles {
            bold: self.bold ^ other.bold,
            italic: self.italic ^ other.italic,
            strike: self.strike ^ other.strike,
        }
    }

    pub fn apply_over(self, base: Style) -> Style {
        Style {
            bold: base.bold ^ self.bold,
            italic: base.italic ^ self.italic,
            strike: base.strike ^ self.strike,
            // Toggles flip nothing else; presentation rides through intact.
            ..base
        }
    }
}

/// TULA FORK: interns font-family names so `Style` can carry a `Copy` id
/// instead of a string. Interior mutability because interning happens inside
/// chain walks that only hold `&self`.
#[derive(Default)]
pub struct FontTable {
    names: RefCell<Vec<String>>,
    /// The theme's minor (body) and major (headings) Latin faces, interned.
    /// `w:asciiTheme`/`w:hAnsiTheme` resolve through these - and docDefaults
    /// very often carries ONLY a theme reference, so without them a document
    /// styled entirely through the theme has no fonts at all.
    theme_minor: RefCell<Option<FontId>>,
    theme_major: RefCell<Option<FontId>>,
}

impl FontTable {
    pub fn intern(&self, name: &str) -> Option<FontId> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let mut names = self.names.borrow_mut();
        if let Some(index) = names.iter().position(|n| n == name) {
            return Some(FontId(index as u16));
        }
        // u16 bounds the table at 65k distinct family names; a document
        // naming more than that is not a document, it is an attack.
        if names.len() >= u16::MAX as usize {
            return None;
        }
        names.push(name.to_string());
        Some(FontId((names.len() - 1) as u16))
    }

    pub fn into_names(self) -> Vec<String> {
        self.names.into_inner()
    }

    /// Record the theme's Latin faces (from a:fontScheme in the theme part).
    pub fn set_theme(&self, minor: Option<&str>, major: Option<&str>) {
        *self.theme_minor.borrow_mut() = minor.and_then(|n| self.intern(n));
        *self.theme_major.borrow_mut() = major.and_then(|n| self.intern(n));
    }

    /// The face an ST_Theme value names. `minor*` is the body face,
    /// `major*` the heading face; the ascii/hAnsi/eastAsia/bidi variants all
    /// map onto the same Latin faces here.
    pub fn theme_font(&self, theme_value: &str) -> Option<FontId> {
        if theme_value.starts_with("minor") {
            *self.theme_minor.borrow()
        } else if theme_value.starts_with("major") {
            *self.theme_major.borrow()
        } else {
            None
        }
    }
}

pub struct Styles<'a> {
    chains: StyleChains<'a, Element>,
    /// TULA FORK: the default paragraph style's id (w:default="1"), which
    /// every paragraph WITHOUT an explicit pStyle inherits from.
    default_para_style: Option<&'a str>,
    /// docDefaults as absolute values (the base the toggles flip over).
    pub doc_defaults: Style,
    /// TULA FORK: docDefaults paragraph presentation (w:pPrDefault) - the
    /// root layer under every paragraph style chain.
    pub doc_default_para: ParaProps,
}

impl<'a> Styles<'a> {
    pub fn parse_opt(root: Option<&'a Element>, fonts: &FontTable) -> Styles<'a> {
        match root {
            Some(root) => Styles::parse(root, fonts),
            None => Styles {
                chains: StyleChains::default(),
                default_para_style: None,
                doc_defaults: Style::PLAIN,
                doc_default_para: ParaProps::default(),
            },
        }
    }

    pub fn parse(root: &'a Element, fonts: &FontTable) -> Styles<'a> {
        let mut chains = StyleChains::default();
        let mut default_para_style = None;
        for style in root.find_all(ns::W, "style") {
            if let Some(id) = style.attr(ns::W, "styleId") {
                let parent = style.find(ns::W, "basedOn").and_then(|e| e.attr(ns::W, "val"));
                chains.insert(id, style, parent);
                if style.attr(ns::W, "type") == Some("paragraph")
                    && matches!(style.attr(ns::W, "default"), Some("1" | "true"))
                {
                    default_para_style = Some(id);
                }
            }
        }
        let doc_defaults = root
            .find(ns::W, "docDefaults")
            .and_then(|d| d.find(ns::W, "rPrDefault"))
            .and_then(|d| d.find(ns::W, "rPr"))
            .map(|rpr| rpr_delta(rpr, fonts).resolve())
            .unwrap_or(Style::PLAIN);
        let doc_default_para = root
            .find(ns::W, "docDefaults")
            .and_then(|d| d.find(ns::W, "pPrDefault"))
            .and_then(|d| d.find(ns::W, "pPr"))
            .map(ppr_props)
            .unwrap_or_default();
        Styles { chains, default_para_style, doc_defaults, doc_default_para }
    }

    /// The parity of `true` toggle specifications along a style's `basedOn`
    /// chain. A `false` in a style leaves the inherited value unchanged.
    pub fn run_toggles(&self, id: &str) -> Result<Toggles, ConvertError> {
        let mut parity = Toggles::default();
        self.chains.walk::<()>(id, |style| {
            if let Some(rpr) = style.find(ns::W, "rPr") {
                parity = parity.xor(Toggles {
                    bold: on_off(rpr, "b") == Some(true),
                    italic: on_off(rpr, "i") == Some(true),
                    strike: on_off(rpr, "strike") == Some(true)
                        || on_off(rpr, "dstrike") == Some(true),
                });
            }
            None
        })?;
        Ok(parity)
    }

    /// TULA FORK: presentation along a style's `basedOn` chain. Plain
    /// last-writer-wins inheritance: the walk visits child-to-root, and for
    /// each property the first (nearest) specification is kept.
    pub fn run_pres(&self, id: &str, fonts: &FontTable) -> Result<StyleDelta, ConvertError> {
        let mut acc = StyleDelta::default();
        self.chains.walk::<()>(id, |style| {
            if let Some(rpr) = style.find(ns::W, "rPr") {
                // `acc` is nearer the child than `rpr` here, so acc wins.
                acc = rpr_pres(rpr, fonts).merge(acc);
            }
            None
        })?;
        Ok(acc)
    }

    /// TULA FORK: paragraph presentation along a style's `basedOn` chain,
    /// over the docDefaults layer. Nearest specification wins per field;
    /// the caller overlays direct pPr formatting on the result.
    /// The chain layer for a paragraph: its own pStyle, or the DEFAULT
    /// paragraph style when it names none - an unstyled paragraph in Word is
    /// not unstyled, it is "Normal".
    pub fn para_props_for(&self, pstyle_id: Option<&str>) -> Result<ParaProps, ConvertError> {
        match pstyle_id.or(self.default_para_style) {
            Some(id) => self.para_props(id),
            None => Ok(self.doc_default_para),
        }
    }

    pub fn para_props(&self, id: &str) -> Result<ParaProps, ConvertError> {
        let mut acc = ParaProps::default();
        self.chains.walk::<()>(id, |style| {
            if let Some(ppr) = style.find(ns::W, "pPr") {
                // `acc` is nearer the child, so acc's fields win.
                acc = merge_para(ppr_props(ppr), acc);
            }
            None
        })?;
        Ok(merge_para(self.doc_default_para, acc))
    }

    /// Heading level a paragraph style resolves to, from its name
    /// (`heading N`, `Title`) or an `outlineLvl`, inherited through
    /// `basedOn`. Tri-state: `Some(None)` when the nearest specification is
    /// the explicit off value (`outlineLvl` 9), which stops inheritance.
    pub fn heading_level(&self, id: &str) -> Result<Option<Option<u8>>, ConvertError> {
        self.chains.walk(id, |style| {
            let name = style
                .find(ns::W, "name")
                .and_then(|e| e.attr(ns::W, "val"))
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(rest) = name.strip_prefix("heading ")
                && let Ok(level) = rest.trim().parse::<u8>()
            {
                return Some(Some(level));
            }
            if name == "title" {
                return Some(Some(1));
            }
            let level = style
                .find(ns::W, "pPr")?
                .find(ns::W, "outlineLvl")?
                .attr(ns::W, "val")?
                .parse::<u8>()
                .ok()?;
            Some(if level < 9 { Some(level + 1) } else { None })
        })
    }

    /// The `numId` a paragraph style contributes, inherited through
    /// `basedOn`. An `ilvl` inside a style's `numPr` is ignored per ECMA-376
    /// §17.3.1.19 - the effective level comes from the abstract levels'
    /// `w:pStyle` bindings ([`Styles::style_numbering_level`]).
    pub fn style_num_pr(&self, id: &str) -> Result<Option<u64>, ConvertError> {
        self.chains.walk(id, |style| {
            style
                .find(ns::W, "pPr")?
                .find(ns::W, "numPr")?
                .find(ns::W, "numId")
                .and_then(|e| e.attr(ns::W, "val"))?
                .parse()
                .ok()
        })
    }

    /// The numbering level a paragraph style binds to: the first style along
    /// the `basedOn` chain (child first) that one of the instance's abstract
    /// levels references via `w:pStyle`.
    pub fn style_numbering_level(
        &self,
        id: &str,
        instance: &crate::formats::docx::numbering::Instance,
    ) -> Result<Option<usize>, ConvertError> {
        self.chains.walk(id, |style| {
            let style_id = style.attr(ns::W, "styleId")?;
            instance.style_level(style_id)
        })
    }

    /// The `numId` referenced by a numbering style's own `numPr`
    /// (`numStyleLink` contract), without inheritance.
    pub fn direct_num_id(&self, id: &str) -> Option<u64> {
        let style = self.chains.definition(id)?;
        style
            .find(ns::W, "pPr")?
            .find(ns::W, "numPr")?
            .find(ns::W, "numId")?
            .attr(ns::W, "val")?
            .parse()
            .ok()
    }
}

/// TULA FORK: overlay `child` on `base`, per field.
pub fn merge_para(base: ParaProps, child: ParaProps) -> ParaProps {
    ParaProps {
        align: child.align.or(base.align),
        indent_start: child.indent_start.or(base.indent_start),
        indent_end: child.indent_end.or(base.indent_end),
        indent_first_line: child.indent_first_line.or(base.indent_first_line),
        indent_hanging: child.indent_hanging.or(base.indent_hanging),
        spacing_before: child.spacing_before.or(base.spacing_before),
        spacing_after: child.spacing_after.or(base.spacing_after),
        line_240ths: child.line_240ths.or(base.line_240ths),
    }
}

/// TULA FORK: the paragraph presentation a `w:pPr` element specifies
/// directly (w:jc, w:ind, w:spacing). Shared by direct formatting, the
/// style chain and docDefaults - the same element shape appears in all
/// three places.
pub fn ppr_props(ppr: &Element) -> ParaProps {
    let mut props = ParaProps::default();
    if let Some(jc) = ppr.find(ns::W, "jc").and_then(|e| e.attr(ns::W, "val")) {
        props.align = Align::parse(jc);
    }
    if let Some(ind) = ppr.find(ns::W, "ind") {
        let read = |names: &[&str]| {
            names.iter().find_map(|n| ind.attr(ns::W, n)).and_then(|v| v.parse().ok())
        };
        props.indent_start = read(&["start", "left"]);
        props.indent_end = read(&["end", "right"]);
        props.indent_first_line = read(&["firstLine"]);
        props.indent_hanging = read(&["hanging"]);
    }
    if let Some(spacing) = ppr.find(ns::W, "spacing") {
        let read = |n: &str| spacing.attr(ns::W, n).and_then(|v| v.parse().ok());
        props.spacing_before = read("before");
        props.spacing_after = read("after");
        // w:line is a straight multiple only under lineRule="auto"; exact and
        // atLeast are absolute heights a flow renderer cannot honour.
        if spacing.attr(ns::W, "lineRule").is_none_or(|r| r == "auto") {
            props.line_240ths = read("line");
        }
    }
    props
}

/// A `w:rPr` element as a tri-state delta - used only for *direct* run
/// formatting, where specifications are absolute on/off.
pub fn rpr_delta(rpr: &Element, fonts: &FontTable) -> StyleDelta {
    let (s, d) = (on_off(rpr, "strike"), on_off(rpr, "dstrike"));
    StyleDelta {
        bold: on_off(rpr, "b"),
        italic: on_off(rpr, "i"),
        strike: if s.is_some() || d.is_some() {
            Some(s.unwrap_or(false) || d.unwrap_or(false))
        } else {
            None
        },
        code: None,
        ..rpr_pres(rpr, fonts)
    }
}

/// TULA FORK: the presentation half of a `w:rPr`, as a delta. Unlike the
/// toggles these are plain properties - in the style chain the nearest
/// specification wins - so the same delta serves direct formatting and
/// chain layers alike.
pub fn rpr_pres(rpr: &Element, fonts: &FontTable) -> StyleDelta {
    let attr = |name: &str| rpr.find(ns::W, name).and_then(|e| e.attr(ns::W, "val"));
    StyleDelta {
        // w:ascii names the Latin-script face; hAnsi is the common fallback a
        // producer writes when ascii is absent; failing both, a theme
        // reference (w:asciiTheme="minorHAnsi") resolves through the theme's
        // fontScheme, which is how most Word defaults name Calibri.
        font: rpr.find(ns::W, "rFonts").and_then(|f| {
            f.attr(ns::W, "ascii")
                .or_else(|| f.attr(ns::W, "hAnsi"))
                .and_then(|name| fonts.intern(name))
                .or_else(|| {
                    f.attr(ns::W, "asciiTheme")
                        .or_else(|| f.attr(ns::W, "hAnsiTheme"))
                        .and_then(|value| fonts.theme_font(value))
                })
        }),
        // w:u is NOT a toggle: any pattern value underlines, `none` is the
        // explicit off. A bare <w:u/> has no defined pattern; treat as unset.
        underline: attr("u").map(|v| v != "none"),
        size_half_points: attr("sz").and_then(|v| v.parse().ok()),
        // `auto` is an explicit reset - "the reader's default colour" - and
        // must override an inherited colour, hence Some(None).
        color: attr("color")
            .map(|v| if v == "auto" { None } else { crate::model::parse_hex_color(v) }),
        highlight: attr("highlight")
            .map(|v| if v == "none" { None } else { crate::model::Highlight::parse(v) }),
        vert_align: attr("vertAlign").map(|v| match v {
            "superscript" => Some(crate::model::VertAlign::Superscript),
            "subscript" => Some(crate::model::VertAlign::Subscript),
            _ => None, // "baseline": explicit reset
        }),
        caps: match (on_off(rpr, "caps"), on_off(rpr, "smallCaps")) {
            (Some(true), _) => Some(Some(crate::model::Caps::All)),
            (_, Some(true)) => Some(Some(crate::model::Caps::Small)),
            (Some(false), _) | (_, Some(false)) => Some(None),
            (None, None) => None,
        },
        ..StyleDelta::default()
    }
}

/// ST_OnOff: `1`/`true`/`on` (or no value) are true; `0`/`false`/`off` are
/// false; an absent element is unspecified.
pub fn on_off(parent: &Element, name: &str) -> Option<bool> {
    let elem = parent.find(ns::W, name)?;
    Some(!matches!(elem.attr(ns::W, "val"), Some("0" | "false" | "off" | "none")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::xml::parse_xml;

    // parse_xml returns a synthetic root wrapping the document element.
    fn parse(doc: &str) -> Element {
        parse_xml(doc.as_bytes()).unwrap()
    }

    #[test]
    fn on_off_covers_the_full_value_space() {
        for (xml, expect) in [
            (r#"<w:b/>"#, Some(true)),
            (r#"<w:b w:val="1"/>"#, Some(true)),
            (r#"<w:b w:val="true"/>"#, Some(true)),
            (r#"<w:b w:val="on"/>"#, Some(true)),
            (r#"<w:b w:val="0"/>"#, Some(false)),
            (r#"<w:b w:val="false"/>"#, Some(false)),
            (r#"<w:b w:val="off"/>"#, Some(false)),
            ("", None),
        ] {
            let root = parse(&format!(
                r#"<w:rPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{xml}</w:rPr>"#
            ));
            let rpr = root.find(ns::W, "rPr").unwrap();
            assert_eq!(on_off(rpr, "b"), expect, "for {xml:?}");
        }
    }

    #[test]
    fn toggles_flip_the_base_and_double_flips_cancel() {
        let base = Style { bold: true, ..Style::PLAIN };
        let flip = Toggles { bold: true, ..Default::default() };
        assert!(!flip.apply_over(base).bold);
        assert!(flip.xor(flip).apply_over(base).bold);
    }

    #[test]
    fn style_false_contributes_nothing_to_parity() {
        let styles_xml = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:style w:type="character" w:styleId="NotBold"><w:rPr><w:b w:val="0"/></w:rPr></w:style>
            <w:style w:type="character" w:styleId="Flip"><w:basedOn w:val="NotBold"/><w:rPr><w:b/></w:rPr></w:style>
        </w:styles>"#;
        let root = parse(styles_xml);
        let styles = Styles::parse(root.find(ns::W, "styles").unwrap(), &FontTable::default());
        assert_eq!(styles.run_toggles("NotBold").unwrap(), Toggles::default());
        assert!(styles.run_toggles("Flip").unwrap().bold);
    }
}
