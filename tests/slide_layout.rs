//! TULA FORK: fixed slide layouts (`Document::slides`) for presentations.
//! Drawing fidelity is checked against PowerPoint by hand; these tests pin
//! the contract a renderer relies on.

mod common;

use anydoc::model::{Block, Document, Fill, Inline, Rgba, ShapeContent};
use common::fixture_root;

fn document(name: &str) -> Document {
    let bytes = std::fs::read(fixture_root().join("pptx").join(name)).unwrap();
    anydoc::to_document(&bytes, Some(anydoc::Format::Pptx)).unwrap()
}

fn text_of(content: &ShapeContent) -> String {
    match content {
        ShapeContent::Text(t) => {
            t.paragraphs.iter().flat_map(|p| p.runs.iter().map(|r| r.text.as_str())).collect()
        }
        _ => String::new(),
    }
}

#[test]
fn every_slide_gets_a_layout_paired_by_its_anchor() {
    let doc = document("pres.pptx");
    let anchors: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(inlines) => match inlines.as_slice() {
                [Inline::Anchor(id)] => Some(id.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let layouts: Vec<&str> = doc.slides.iter().map(|s| s.anchor.as_str()).collect();
    assert_eq!(layouts, ["slide-1", "slide-2"]);
    assert_eq!(layouts, anchors);
    for slide in &doc.slides {
        assert_eq!((slide.width, slide.height), (10080625.0, 5670550.0));
    }
}

#[test]
fn shapes_keep_their_position_fill_and_text() {
    let doc = document("pres.pptx");
    let title = &doc.slides[0].shapes[0];
    assert_eq!(text_of(&title.content), "Deck Title Slide");
    assert_eq!((title.frame.x, title.frame.y), (720000.0, 360000.0));

    // A text box inside a group: the group's child space maps 1:1 here, so
    // the frame comes through unchanged, fill and run size intact.
    let grouped = doc.slides[1]
        .shapes
        .iter()
        .find(|s| text_of(&s.content).contains("Inside a group shape"))
        .expect("the grouped text box is drawn");
    let f = grouped.frame;
    assert_eq!((f.x, f.y, f.width, f.height), (720000.0, 6480000.0, 2880000.0, 720000.0));
    assert_eq!(grouped.fill, Some(Fill::Solid(Rgba { rgb: [0x72, 0x9F, 0xCF], alpha: 1.0 })));
    let ShapeContent::Text(text) = &grouped.content else { unreachable!() };
    assert_eq!(text.paragraphs[0].runs[0].size, 1800.0);
}

#[test]
fn a_slide_whose_content_has_no_position_gets_no_layout() {
    // handmade-inherit's placeholders carry no xfrm on the slide, layout or
    // master. Drawing the slide would drop its text, so it has no layout and
    // a reader keeps the flowing text instead.
    let doc = document("handmade-inherit.pptx");
    assert!(doc.slides.is_empty());
    assert!(!doc.blocks.is_empty());
}
