/// Fully resolved character style. Tri-state deltas exist only during
/// frontend resolution (`shared::delta`); by the time content reaches the
/// model every toggle has a definite value.
///
/// TULA FORK: alongside the four Markdown-visible toggles, `Style` carries
/// PRESENTATION - properties Markdown cannot express but a native renderer
/// can. They ride inside `Style` (rather than a parallel structure) so they
/// flow through the existing delta/chain resolution untouched, and they are
/// all `Copy` so `Style` stays `Copy`. The Markdown serializer projects them
/// away at its normalization boundary (`emphasis_only`), which is what keeps
/// upstream's byte-for-byte output - and its snapshot suite - unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Bold weight.
    pub bold: bool,
    /// Italic or oblique.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// Monospace, from a code or teletype character style.
    pub code: bool,

    // --- presentation (Tula fork) ---------------------------------------
    /// Underlined (`w:u` with any pattern other than `none`).
    pub underline: bool,
    /// Font size in half-points (`w:sz`), the unit the source uses.
    pub size_half_points: Option<u16>,
    /// Text colour as RGB. `auto` in the source resolves to `None`.
    pub color: Option<[u8; 3]>,
    /// Highlight colour (`w:highlight`, a closed 16-value enum in ECMA-376).
    pub highlight: Option<Highlight>,
    /// Superscript / subscript.
    pub vert_align: Option<VertAlign>,
    /// All-caps or small-caps rendering.
    pub caps: Option<Caps>,
    /// Font family, as an index into [`Document::fonts`](crate::model::Document::fonts).
    /// Interned so `Style` stays `Copy`; the table carries the author's own
    /// names ("Times New Roman"), mapping them to a shippable face is the
    /// renderer's job.
    pub font: Option<FontId>,
}

/// Index into [`Document::fonts`](crate::model::Document::fonts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontId(pub u16);

impl Style {
    /// No toggle set.
    pub const PLAIN: Style = Style {
        bold: false,
        italic: false,
        strike: false,
        code: false,
        underline: false,
        size_half_points: None,
        color: None,
        highlight: None,
        vert_align: None,
        caps: None,
        font: None,
    };

    /// The Markdown-visible projection: toggles kept, presentation dropped.
    /// The serializer normalizes through this so two runs differing only in
    /// presentation still merge into one emphasis span.
    pub fn emphasis_only(self) -> Style {
        Style {
            bold: self.bold,
            italic: self.italic,
            strike: self.strike,
            code: self.code,
            ..Style::PLAIN
        }
    }
}

/// ST_HighlightColor: the only 16 values Word will ever write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    /// `yellow`.
    Yellow,
    /// `green`.
    Green,
    /// `cyan`.
    Cyan,
    /// `magenta`.
    Magenta,
    /// `blue`.
    Blue,
    /// `red`.
    Red,
    /// `darkBlue`.
    DarkBlue,
    /// `darkCyan`.
    DarkCyan,
    /// `darkGreen`.
    DarkGreen,
    /// `darkMagenta`.
    DarkMagenta,
    /// `darkRed`.
    DarkRed,
    /// `darkYellow`.
    DarkYellow,
    /// `darkGray`.
    DarkGray,
    /// `lightGray`.
    LightGray,
    /// `black`.
    Black,
    /// `white`.
    White,
}

impl Highlight {
    /// Parse an ST_HighlightColor name. `none` and anything outside the enum
    /// return `None` - better to drop a colour than to invent one.
    pub fn parse(name: &str) -> Option<Highlight> {
        Some(match name {
            "yellow" => Highlight::Yellow,
            "green" => Highlight::Green,
            "cyan" => Highlight::Cyan,
            "magenta" => Highlight::Magenta,
            "blue" => Highlight::Blue,
            "red" => Highlight::Red,
            "darkBlue" => Highlight::DarkBlue,
            "darkCyan" => Highlight::DarkCyan,
            "darkGreen" => Highlight::DarkGreen,
            "darkMagenta" => Highlight::DarkMagenta,
            "darkRed" => Highlight::DarkRed,
            "darkYellow" => Highlight::DarkYellow,
            "darkGray" => Highlight::DarkGray,
            "lightGray" => Highlight::LightGray,
            "black" => Highlight::Black,
            "white" => Highlight::White,
            _ => return None,
        })
    }

    /// The source name, for bindings that hand the value to a renderer.
    pub fn name(self) -> &'static str {
        match self {
            Highlight::Yellow => "yellow",
            Highlight::Green => "green",
            Highlight::Cyan => "cyan",
            Highlight::Magenta => "magenta",
            Highlight::Blue => "blue",
            Highlight::Red => "red",
            Highlight::DarkBlue => "darkBlue",
            Highlight::DarkCyan => "darkCyan",
            Highlight::DarkGreen => "darkGreen",
            Highlight::DarkMagenta => "darkMagenta",
            Highlight::DarkRed => "darkRed",
            Highlight::DarkYellow => "darkYellow",
            Highlight::DarkGray => "darkGray",
            Highlight::LightGray => "lightGray",
            Highlight::Black => "black",
            Highlight::White => "white",
        }
    }
}

/// Vertical run alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertAlign {
    /// Raised, smaller.
    Superscript,
    /// Lowered, smaller.
    Subscript,
}

/// Capitalization rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caps {
    /// Render as all capitals (`w:caps`).
    All,
    /// Render as small capitals (`w:smallCaps`).
    Small,
}

/// Parse a WordprocessingML hex colour (`RRGGBB`). `auto` - "whatever the
/// reader's default is" - resolves to `None`, never to black.
pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    if value.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}
