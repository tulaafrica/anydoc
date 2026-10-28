//! TULA FORK: fixed-layout slide geometry.
//!
//! The block stream flattens a slide into reading order, which is what
//! Markdown and a reflowing reader want. A renderer that draws the slide as
//! PowerPoint does needs the opposite: every shape at its own position, in
//! paint order, with its fill, outline and fully resolved text. This module
//! carries that, alongside the blocks rather than instead of them, so the
//! Markdown output (and upstream's snapshot suite) is untouched.
//!
//! Units are the source's own: EMU for lengths (914400 per inch, 12700 per
//! point), hundredths of a point for font sizes and letter spacing.
//! Everything is resolved - theme colours, placeholder inheritance from the
//! slide layout and master, style references, autofit scaling - so a
//! renderer only scales and draws.

use crate::model::{AnchorId, Chart, ImageSource};

/// One slide's drawable layout.
#[derive(Debug, Clone)]
pub struct SlideLayout {
    /// The slide's start anchor (`slide-N`), the same id the block stream
    /// opens the slide with - how a consumer pairs the two.
    pub anchor: AnchorId,
    /// Slide width, EMU.
    pub width: f64,
    /// Slide height, EMU.
    pub height: f64,
    /// The slide background, inherited from the layout or master when the
    /// slide sets none. `None` means white.
    pub background: Option<Fill>,
    /// Shapes in paint order (back to front): master decoration, then layout
    /// decoration, then the slide's own shapes. Groups are flattened.
    pub shapes: Vec<SlideShape>,
}

/// An RGB colour with opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    /// Red, green, blue.
    pub rgb: [u8; 3],
    /// Opacity, 0 (transparent) to 1 (opaque).
    pub alpha: f32,
}

/// How an area is painted.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    /// One colour.
    Solid(Rgba),
    /// A gradient.
    Gradient {
        /// Linear angle in degrees, clockwise from left-to-right (DrawingML's
        /// convention: 90 runs top to bottom).
        angle: f64,
        /// A path (radial) gradient rather than a linear one.
        radial: bool,
        /// Stops as (position 0..1, colour), in order.
        stops: Vec<(f64, Rgba)>,
    },
    /// A picture stretched over the area.
    Image(ImageSource),
}

/// Dash pattern of an outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDash {
    /// Continuous.
    Solid,
    /// Dashes.
    Dash,
    /// Dots.
    Dot,
    /// Alternating dash and dot.
    DashDot,
}

/// An outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Stroke width, EMU.
    pub width: f64,
    /// Stroke colour.
    pub color: Rgba,
    /// Dash pattern.
    pub dash: LineDash,
    /// Arrowhead at the start (`triangle`, `arrow`, `stealth`, `diamond`,
    /// `oval`), for connectors.
    pub head_end: Option<String>,
    /// Arrowhead at the end.
    pub tail_end: Option<String>,
}

/// An outer drop shadow.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    /// Horizontal offset, EMU.
    pub dx: f64,
    /// Vertical offset, EMU.
    pub dy: f64,
    /// Blur radius, EMU.
    pub blur: f64,
    /// Shadow colour.
    pub color: Rgba,
}

/// Where a shape sits, on the slide's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Left edge, EMU.
    pub x: f64,
    /// Top edge, EMU.
    pub y: f64,
    /// Width, EMU.
    pub width: f64,
    /// Height, EMU.
    pub height: f64,
    /// Clockwise rotation about the centre, degrees.
    pub rotation: f64,
    /// Mirrored left-right.
    pub flip_h: bool,
    /// Mirrored top-bottom.
    pub flip_v: bool,
}

/// A shape's outline geometry.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// The frame's rectangle.
    Rect,
    /// A rectangle with rounded corners.
    RoundRect {
        /// Corner radius as a fraction of the shorter side (0..0.5).
        radius: f64,
    },
    /// The ellipse inscribed in the frame.
    Ellipse,
    /// A straight line from the frame's top-left to bottom-right corner
    /// (flips choose the other diagonal).
    Line,
    /// Any other outline (preset shapes beyond the four above, freeforms),
    /// in the frame's own coordinate space: (0,0) is the frame's top-left,
    /// units are EMU. Sub-paths drawn in opposite directions cut holes
    /// (non-zero winding).
    Path {
        /// The outline, as SVG-style commands.
        commands: Vec<PathCmd>,
        /// Whether the path is filled (open freeform strokes are not).
        filled: bool,
    },
}

/// One outline command; coordinates in EMU, relative to the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    /// Start a sub-path.
    MoveTo(f64, f64),
    /// Straight line.
    LineTo(f64, f64),
    /// Cubic Bezier: two control points, then the end point.
    CubicTo(f64, f64, f64, f64, f64, f64),
    /// Quadratic Bezier: control point, then the end point.
    QuadTo(f64, f64, f64, f64),
    /// Elliptical arc, as SVG's `A` command (no axis rotation).
    ArcTo {
        /// Horizontal radius.
        rx: f64,
        /// Vertical radius.
        ry: f64,
        /// Take the longer way round.
        large: bool,
        /// Clockwise.
        sweep: bool,
        /// End x.
        x: f64,
        /// End y.
        y: f64,
    },
    /// Close the sub-path.
    Close,
}

/// One drawable shape.
#[derive(Debug, Clone)]
pub struct SlideShape {
    /// Position and size.
    pub frame: Frame,
    /// Outline geometry.
    pub geometry: Geometry,
    /// The DrawingML preset name the geometry came from (`rect`,
    /// `chevron`, ...), or `custom` for a freeform. Informational: a
    /// renderer draws [`SlideShape::geometry`].
    pub preset: String,
    /// Area fill, `None` when unfilled.
    pub fill: Option<Fill>,
    /// Outline, `None` when there is none.
    pub line: Option<Line>,
    /// Outer shadow.
    pub shadow: Option<Shadow>,
    /// What the shape holds besides its own outline.
    pub content: ShapeContent,
}

/// The content drawn inside a shape's frame.
#[derive(Debug, Clone)]
pub enum ShapeContent {
    /// Nothing: a plain shape.
    None,
    /// Text.
    Text(TextFrame),
    /// A picture.
    Image {
        /// The picture's bytes.
        source: ImageSource,
        /// Fractions cropped off each edge (negative values pad).
        crop: Crop,
    },
    /// A table.
    Table(SlideTable),
    /// A chart, drawn by the renderer from its data.
    Chart(Chart),
}

/// Fractions of a picture cropped off each edge.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Crop {
    /// Left.
    pub left: f64,
    /// Top.
    pub top: f64,
    /// Right.
    pub right: f64,
    /// Bottom.
    pub bottom: f64,
}

/// Vertical placement of text inside its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    /// Top.
    Top,
    /// Centred.
    Middle,
    /// Bottom.
    Bottom,
}

/// Text laid out in a box.
#[derive(Debug, Clone)]
pub struct TextFrame {
    /// Inner margins left, top, right, bottom, EMU.
    pub insets: [f64; 4],
    /// Vertical placement.
    pub anchor: TextAnchor,
    /// Whether lines wrap at the box width (`false`: one line per paragraph).
    pub wrap: bool,
    /// The paragraphs, autofit scaling already applied.
    pub paragraphs: Vec<SlideParagraph>,
}

/// Horizontal alignment of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideAlign {
    /// Left.
    Left,
    /// Centred.
    Center,
    /// Right.
    Right,
    /// Justified.
    Justify,
}

/// Paragraph spacing, in one of DrawingML's two units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    /// A multiple of the line: 1.0 is single spacing, which PowerPoint draws
    /// as 1.2 x the font size.
    Lines(f64),
    /// Absolute, in hundredths of a point.
    Points(f64),
}

/// A paragraph's marker.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideBullet {
    /// The marker text, numbering already resolved (`•`, `3.`, `iv)`).
    pub text: String,
    /// Marker colour; `None` follows the first run.
    pub color: Option<Rgba>,
    /// Marker font; `None` follows the first run.
    pub font: Option<String>,
    /// Marker size relative to the first run (1.0 = same).
    pub size: f64,
}

/// One paragraph of slide text.
#[derive(Debug, Clone)]
pub struct SlideParagraph {
    /// Horizontal alignment.
    pub align: SlideAlign,
    /// Left margin, EMU.
    pub margin_left: f64,
    /// First-line indent relative to the margin, EMU (negative hangs).
    pub indent: f64,
    /// Line spacing; `None` is single.
    pub line_spacing: Option<Spacing>,
    /// Space above.
    pub space_before: Option<Spacing>,
    /// Space below.
    pub space_after: Option<Spacing>,
    /// The marker, if any.
    pub bullet: Option<SlideBullet>,
    /// The text runs. A line break is a run whose text is `"\n"`.
    pub runs: Vec<SlideRun>,
    /// Font size of the paragraph mark, hundredths of a point: the height
    /// an empty paragraph occupies.
    pub end_size: f64,
}

/// One run of identically styled slide text.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideRun {
    /// The text. May contain `\n` (a break, or a vertical tab in the source).
    pub text: String,
    /// Font size, hundredths of a point.
    pub size: f64,
    /// Font family as the author named it ("Calibri"); theme references are
    /// resolved. Mapping to a shippable face is the renderer's job.
    pub font: Option<String>,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
    /// Struck through.
    pub strike: bool,
    /// Text colour.
    pub color: Rgba,
    /// Extra space between characters, hundredths of a point.
    pub letter_spacing: f64,
    /// Baseline shift as a fraction of the font size (0.3 = superscript).
    pub baseline: f64,
    /// Rendered in capitals.
    pub all_caps: bool,
}

/// A slide table.
#[derive(Debug, Clone)]
pub struct SlideTable {
    /// Column widths, EMU.
    pub columns: Vec<f64>,
    /// Rows, top to bottom.
    pub rows: Vec<SlideTableRow>,
}

/// One table row.
#[derive(Debug, Clone)]
pub struct SlideTableRow {
    /// Minimum height, EMU (text may grow it).
    pub height: f64,
    /// The cells that start in this row. Cells covered by a merge are left
    /// out; each cell names its own column.
    pub cells: Vec<SlideTableCell>,
}

/// One table cell, table style already applied.
#[derive(Debug, Clone)]
pub struct SlideTableCell {
    /// Index of the grid column the cell starts in.
    pub col: usize,
    /// Columns spanned.
    pub col_span: usize,
    /// Rows spanned.
    pub row_span: usize,
    /// Cell fill.
    pub fill: Option<Fill>,
    /// Borders left, top, right, bottom.
    pub borders: [Option<Line>; 4],
    /// Inner margins left, top, right, bottom, EMU.
    pub insets: [f64; 4],
    /// Vertical placement.
    pub anchor: TextAnchor,
    /// The cell's text.
    pub paragraphs: Vec<SlideParagraph>,
}
