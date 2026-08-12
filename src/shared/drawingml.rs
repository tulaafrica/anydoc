//! Textual extraction of DrawingML rich objects shared by DOCX and PPTX:
//! charts (title, axis titles, cached series data) and SmartArt diagram
//! data (text points).

use crate::model::{Block, Chart, ChartKind, ChartSeries, Inline, List, ListItem};
use crate::package::xml::{Element, ns};
use crate::shared::text::clean_text;

/// A chart part as a single [`Block::Chart`] carrying the cached data in
/// typed form. Markdown renders it through [`Chart::fallback_blocks`] (bold
/// title paragraph plus a categories x series table), so text output is
/// unchanged; drawing renderers get kind, categories and numeric series.
pub fn chart_blocks(root: &Element) -> Vec<Block> {
    let title = root
        .first_descendant(ns::CHART, "title")
        .map(|t| clean_text(&drawing_text(t)))
        .filter(|t| !t.trim().is_empty());
    let mut categories: Vec<String> = Vec::new();
    let mut series: Vec<ChartSeries> = Vec::new();
    for ser in root.descendants(ns::CHART, "ser") {
        // Cached display strings only; `c:f` formula references are not text.
        let name = ser
            .find(ns::CHART, "tx")
            .and_then(|t| t.first_descendant(ns::CHART, "v"))
            .map(|v| clean_text(&v.text()))
            .unwrap_or_default();
        let cats: Vec<String> = ser
            .find(ns::CHART, "cat")
            .map(|c| c.descendants(ns::CHART, "v").map(|v| clean_text(&v.text())).collect())
            .unwrap_or_default();
        if categories.is_empty() {
            categories = cats;
        }
        let labels: Vec<String> = ser
            .find(ns::CHART, "val")
            .map(|v| v.descendants(ns::CHART, "v").map(|p| clean_text(&p.text())).collect())
            .unwrap_or_default();
        let values = labels.iter().map(|l| l.trim().parse::<f64>().ok()).collect();
        series.push(ChartSeries { name, labels, values });
    }
    let axis_title = root
        .first_descendant(ns::CHART, "catAx")
        .and_then(|ax| ax.find(ns::CHART, "title"))
        .map(|t| clean_text(&drawing_text(t)))
        .unwrap_or_default();
    let chart = Chart { kind: chart_kind(root), title, axis_title, categories, series };
    if chart.is_empty() { Vec::new() } else { vec![Block::Chart(chart)] }
}

/// The plot kind, read from the first recognized plot element in the plot
/// area. A combo chart reports its first plot; 3-D variants collapse onto
/// their flat kind.
fn chart_kind(root: &Element) -> ChartKind {
    let Some(plot_area) = root.first_descendant(ns::CHART, "plotArea") else {
        return ChartKind::Other;
    };
    for child in plot_area.child_elems() {
        let kind = match child.local.as_str() {
            "barChart" | "bar3DChart" => ChartKind::Bar,
            "lineChart" | "line3DChart" | "stockChart" => ChartKind::Line,
            "areaChart" | "area3DChart" => ChartKind::Area,
            "pieChart" | "pie3DChart" | "ofPieChart" => ChartKind::Pie,
            "doughnutChart" => ChartKind::Doughnut,
            "scatterChart" | "bubbleChart" => ChartKind::Scatter,
            "radarChart" => ChartKind::Radar,
            "surface3DChart" | "surfaceChart" => ChartKind::Other,
            _ => continue,
        };
        return kind;
    }
    ChartKind::Other
}

/// A SmartArt data part as a bullet list of its text points in order.
pub fn diagram_blocks(root: &Element) -> Vec<Block> {
    let items: Vec<ListItem> = root
        .descendants(ns::DGM, "pt")
        .filter_map(|pt| {
            let text = clean_text(&pt.find(ns::DGM, "t")?.text());
            if text.trim().is_empty() {
                return None;
            }
            Some(ListItem {
                blocks: vec![Block::Paragraph(vec![Inline::plain(text)])],
                checked: None,
                marker_label: None,
            })
        })
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    vec![Block::List(List { marker: crate::model::MarkerKind::Bullet, start: 1, items })]
}

/// Text runs inside DrawingML rich text (`a:p`/`a:r`/`a:t`), joined.
pub fn drawing_text(elem: &Element) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in elem.descendants(ns::A, "p") {
        let text = p.text();
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    if parts.is_empty() { elem.text() } else { parts.join(" ") }
}
