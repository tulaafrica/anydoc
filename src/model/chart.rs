use crate::model::{Block, Cell, Inline, Style, Table, TableKind};

/// A chart extracted from DrawingML, carrying its cached data in typed form
/// so renderers that can draw (mobile IR consumers) get numbers, while text
/// renderers fall back to the same titled table anydoc always emitted.
#[derive(Debug, Clone)]
pub struct Chart {
    /// Simplified plot kind: `bar`, `line`, `area`, `pie`, `doughnut`,
    /// `scatter`, `radar` — or `other` when the plot area holds something
    /// anydoc does not classify. Combo charts report their first plot.
    pub kind: ChartKind,
    /// Chart title text, cleaned.
    pub title: Option<String>,
    /// Category-axis title; the table fallback uses it as the corner header.
    pub axis_title: String,
    /// Category labels shared by every series.
    pub categories: Vec<String>,
    /// The data series, in document order.
    pub series: Vec<ChartSeries>,
}

/// Simplified plot kind; 3-D and sub-variants collapse onto their flat kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ChartKind {
    Bar,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    Radar,
    Other,
}

impl ChartKind {
    /// The kind as the lowercase string the mobile IR emits.
    pub fn as_str(&self) -> &'static str {
        match self {
            ChartKind::Bar => "bar",
            ChartKind::Line => "line",
            ChartKind::Area => "area",
            ChartKind::Pie => "pie",
            ChartKind::Doughnut => "doughnut",
            ChartKind::Scatter => "scatter",
            ChartKind::Radar => "radar",
            ChartKind::Other => "other",
        }
    }
}

/// One data series: a name plus one point per category.
#[derive(Debug, Clone)]
pub struct ChartSeries {
    /// Series name from the cached `c:tx` value; empty when absent.
    pub name: String,
    /// Cached display strings, one per category — exactly what the table
    /// fallback prints (empty string where the cache has a gap).
    pub labels: Vec<String>,
    /// The same points parsed as numbers; `None` where the cached string is
    /// not numeric or missing.
    pub values: Vec<Option<f64>>,
}

impl Chart {
    /// True when there is nothing to draw and nothing to tabulate.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && (self.series.is_empty() || self.categories.is_empty())
    }

    /// The chart as plain blocks — bold title paragraph plus a categories x
    /// series table. This is the historical anydoc output for charts;
    /// Markdown rendering goes through it so output stays byte-identical.
    pub fn fallback_blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();
        if let Some(title) = &self.title {
            blocks.push(Block::Paragraph(vec![Inline::Text {
                text: title.clone(),
                style: Style { bold: true, ..Style::PLAIN },
            }]));
        }
        if !self.series.is_empty() && !self.categories.is_empty() {
            let mut header: Vec<Cell> =
                vec![Cell::from_inlines(vec![Inline::plain(self.axis_title.clone())])];
            header.extend(
                self.series.iter().map(|s| Cell::from_inlines(vec![Inline::plain(s.name.clone())])),
            );
            let mut rows = vec![header];
            for (i, cat) in self.categories.iter().enumerate() {
                let mut row = vec![Cell::from_inlines(vec![Inline::plain(cat.clone())])];
                for s in &self.series {
                    let v = s.labels.get(i).cloned().unwrap_or_default();
                    row.push(Cell::from_inlines(vec![Inline::plain(v)]));
                }
                rows.push(row);
            }
            blocks.push(Block::Table(Table::from_rows(rows, 1, TableKind::Data)));
        }
        blocks
    }
}
