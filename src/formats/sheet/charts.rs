//! TULA FORK: charts anchored on worksheets and chart sheets, as typed
//! [`Block::Chart`]s (the same `shared::drawingml::chart_blocks` docx and
//! pptx use). Chain: sheet part -> drawing rels -> drawing XML (document
//! order decides chart order) -> chart parts. Every failure inside degrades
//! to "no charts", never to a failed conversion.

use crate::model::Block;
use crate::package::relationships::{read_rels, rels_part_for};
use crate::package::xml::ns;
use crate::package::{Package, path};

/// The charts anchored on one sheet, in the drawing's document order.
pub(super) fn sheet_chart_blocks(pkg: &mut Package, sheet_part: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let Ok(sheet_rels) = read_rels(pkg, &rels_part_for(sheet_part)) else { return blocks };
    let drawing_parts: Vec<String> = sheet_rels
        .iter()
        .filter(|(_, rel)| rel.rel_type.ends_with("/drawing"))
        .filter_map(|(_, rel)| path::resolve(sheet_part, &rel.target).ok().map(|t| t.path))
        .collect();

    for drawing_part in drawing_parts {
        let Ok(Some(drawing)) = pkg.optional_xml_part(&drawing_part) else { continue };
        let Ok(drawing_rels) = read_rels(pkg, &rels_part_for(&drawing_part)) else { continue };
        // Document order of the DRAWING decides chart order, not rels-map order.
        for chart_ref in drawing.descendants(ns::CHART, "chart") {
            let Some(rel_id) = chart_ref.attr_qualified(ns::R, "id") else { continue };
            let Some(target) = drawing_rels.internal_target(rel_id) else { continue };
            let Ok(chart_part) = path::resolve(&drawing_part, target) else { continue };
            match pkg.optional_xml_part(&chart_part.path) {
                Ok(Some(root)) => blocks.extend(crate::shared::drawingml::chart_blocks(&root)),
                _ => log::warn!("skipping unreadable chart part {}", chart_part.path),
            }
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use crate::model::{Block, Inline};
    use std::io::Write;

    /// Minimal xlsx whose one worksheet anchors one chart (title + a cached
    /// series). Exercises the whole chain: workbook -> sheet rels -> drawing
    /// -> chart part -> chart_blocks.
    fn xlsx_with_chart() -> Vec<u8> {
        let parts: &[(&str, &str)] = &[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Revenue" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Month</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Jan</t></is></c><c r="B2"><v>10</v></c></row></sheetData><drawing r:id="rId2"/></worksheet>"#,
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#,
            ),
            (
                "xl/drawings/drawing1.xml",
                r#"<?xml version="1.0"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:oneCellAnchor><xdr:graphicFrame><c:chart r:id="rId1"/></xdr:graphicFrame></xdr:oneCellAnchor></xdr:wsDr>"#,
            ),
            (
                "xl/drawings/_rels/drawing1.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/></Relationships>"#,
            ),
            (
                "xl/charts/chart1.xml",
                r#"<?xml version="1.0"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Monthly Sales</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:barChart><c:ser><c:tx><c:strRef><c:f>Revenue!$B$1</c:f><c:strCache><c:pt idx="0"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strRef><c:f>Revenue!$A$2</c:f><c:strCache><c:pt idx="0"><c:v>Jan</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Revenue!$B$2</c:f><c:numCache><c:pt idx="0"><c:v>10</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#,
            ),
        ];
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in parts {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn charts_follow_their_sheet_as_typed_blocks() {
        let doc = super::super::parse(&xlsx_with_chart()).unwrap();
        // Single sheet: cells table, then one typed chart block whose
        // fallback is the bold title + categories x series table.
        assert!(matches!(doc.blocks.first(), Some(Block::Table(_))), "sheet cells first");
        let Some(Block::Chart(chart)) = doc.blocks.get(1) else {
            panic!("expected a chart block, got {:?}", doc.blocks.get(1));
        };
        assert_eq!(chart.kind, crate::model::ChartKind::Bar);
        assert_eq!(chart.title.as_deref(), Some("Monthly Sales"));
        assert_eq!(chart.categories, vec!["Jan"]);
        assert_eq!(chart.series.len(), 1);
        assert_eq!(chart.series[0].name, "Sales");
        assert_eq!(chart.series[0].values, vec![Some(10.0)]);
        assert_eq!(doc.blocks.len(), 2);

        // The fallback keeps the historical text shape.
        let fallback = chart.fallback_blocks();
        let Some(Block::Paragraph(inlines)) = fallback.first() else {
            panic!("expected the chart title, got {:?}", fallback.first());
        };
        let Some(Inline::Text { text, style }) = inlines.first() else {
            panic!("expected title text");
        };
        assert_eq!(text, "Monthly Sales");
        assert!(style.bold);
        assert!(matches!(fallback.get(1), Some(Block::Table(_))), "chart data table last");
        assert_eq!(fallback.len(), 2);
    }
}
