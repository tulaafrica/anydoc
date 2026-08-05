//! Tri-state style deltas used during cascade resolution. A property is
//! either explicitly on, explicitly off, or unset (inherit); only after the
//! full cascade is a delta collapsed into the model's resolved [`Style`].
//!
//! TULA FORK: the delta carries the presentation properties too. Unlike
//! bold/italic/strike - toggle properties, whose style-chain layers XOR
//! (ECMA-376 §17.7.3) - presentation properties inherit last-writer-wins:
//! the nearest explicit specification along the chain is the value.

use crate::model::{Caps, Highlight, Inline, Style, VertAlign};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleDelta {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    pub code: Option<bool>,

    // --- presentation (Tula fork) ---------------------------------------
    pub underline: Option<bool>,
    pub size_half_points: Option<u16>,
    /// `Some(None)` is an explicit reset (`w:color w:val="auto"`), which must
    /// override an inherited colour rather than reading as unspecified.
    pub color: Option<Option<[u8; 3]>>,
    /// `Some(None)` is an explicit `w:highlight w:val="none"`.
    pub highlight: Option<Option<Highlight>>,
    /// `Some(None)` is an explicit `w:vertAlign w:val="baseline"`.
    pub vert_align: Option<Option<VertAlign>>,
    /// `Some(None)` is an explicit caps-off (`w:caps w:val="0"`).
    pub caps: Option<Option<Caps>>,
}

impl StyleDelta {
    /// Overlay `child` on `self`: an explicit child value (on **or** off)
    /// wins; unset inherits.
    pub fn merge(self, child: StyleDelta) -> StyleDelta {
        StyleDelta {
            bold: child.bold.or(self.bold),
            italic: child.italic.or(self.italic),
            strike: child.strike.or(self.strike),
            code: child.code.or(self.code),
            underline: child.underline.or(self.underline),
            size_half_points: child.size_half_points.or(self.size_half_points),
            color: child.color.or(self.color),
            highlight: child.highlight.or(self.highlight),
            vert_align: child.vert_align.or(self.vert_align),
            caps: child.caps.or(self.caps),
        }
    }

    pub fn apply(self, base: Style) -> Style {
        Style {
            bold: self.bold.unwrap_or(base.bold),
            italic: self.italic.unwrap_or(base.italic),
            strike: self.strike.unwrap_or(base.strike),
            code: self.code.unwrap_or(base.code),
            underline: self.underline.unwrap_or(base.underline),
            size_half_points: self.size_half_points.or(base.size_half_points),
            color: self.color.unwrap_or(base.color),
            highlight: self.highlight.unwrap_or(base.highlight),
            vert_align: self.vert_align.unwrap_or(base.vert_align),
            caps: self.caps.unwrap_or(base.caps),
        }
    }

    pub fn resolve(self) -> Style {
        self.apply(Style::PLAIN)
    }
}

/// Drop from every run the emphasis `base` already carries. A heading style
/// defines its own typography, so its runs should carry only what they add
/// beyond it - otherwise the bold in `## **Heading**` is the style's, not the
/// author's.
pub fn rebase_emphasis(inlines: &mut [Inline], base: Style) {
    if base.emphasis_only() == Style::PLAIN {
        return;
    }
    for inline in inlines {
        match inline {
            Inline::Text { style, .. } => {
                style.bold &= !base.bold;
                style.italic &= !base.italic;
                style.strike &= !base.strike;
            }
            Inline::Link { content, .. } => rebase_emphasis(content, base),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_off_beats_inherited_on() {
        let base = StyleDelta { bold: Some(true), ..Default::default() };
        let child = StyleDelta { bold: Some(false), ..Default::default() };
        assert_eq!(base.merge(child).resolve(), Style::PLAIN);
    }

    #[test]
    fn unset_inherits() {
        let base = StyleDelta { bold: Some(true), italic: Some(true), ..Default::default() };
        let child = StyleDelta { italic: Some(false), ..Default::default() };
        let resolved = base.merge(child).resolve();
        assert!(resolved.bold && !resolved.italic);
    }

    #[test]
    fn presentation_inherits_last_writer_wins() {
        let base = StyleDelta {
            size_half_points: Some(24),
            color: Some(Some([0xff, 0, 0])),
            ..Default::default()
        };
        let child = StyleDelta { size_half_points: Some(48), ..Default::default() };
        let resolved = base.merge(child).resolve();
        assert_eq!(resolved.size_half_points, Some(48)); // child wins
        assert_eq!(resolved.color, Some([0xff, 0, 0])); // unset inherits
    }

    #[test]
    fn explicit_auto_resets_an_inherited_color() {
        let base = StyleDelta { color: Some(Some([0xff, 0, 0])), ..Default::default() };
        let child = StyleDelta { color: Some(None), ..Default::default() };
        assert_eq!(base.merge(child).resolve().color, None);
    }
}
