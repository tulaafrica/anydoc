//! Count the presentation the Tula fork recovers from a real document.
//! Compared against rn-docx-ir's counts for the same file to check parity.
use anydoc::model::{Block, CellSlot, Inline};

#[derive(Default)]
struct Counts {
    runs: usize,
    chars: usize,
    bold: usize,
    italic: usize,
    underline: usize,
    strike: usize,
    size: usize,
    color: usize,
    highlight: usize,
    vert: usize,
    caps: usize,
    tables: usize,
    with_widths: usize,
}

fn walk_inlines(inlines: &[Inline], c: &mut Counts) {
    for i in inlines {
        match i {
            Inline::Text { text, style } => {
                c.runs += 1;
                c.chars += text.chars().count();
                if style.bold {
                    c.bold += 1
                }
                if style.italic {
                    c.italic += 1
                }
                if style.underline {
                    c.underline += 1
                }
                if style.strike {
                    c.strike += 1
                }
                if style.size_half_points.is_some() {
                    c.size += 1
                }
                if style.color.is_some() {
                    c.color += 1
                }
                if style.highlight.is_some() {
                    c.highlight += 1
                }
                if style.vert_align.is_some() {
                    c.vert += 1
                }
                if style.caps.is_some() {
                    c.caps += 1
                }
            }
            Inline::Link { content, .. } => walk_inlines(content, c),
            _ => {}
        }
    }
}

fn walk_blocks(blocks: &[Block], c: &mut Counts) {
    for b in blocks {
        match b {
            Block::Heading { content, .. } => walk_inlines(content, c),
            Block::Paragraph(inlines) => walk_inlines(inlines, c),
            Block::List(list) => {
                for item in &list.items {
                    walk_blocks(&item.blocks, c);
                }
            }
            Block::Table(t) => {
                c.tables += 1;
                if t.column_widths.is_some() {
                    c.with_widths += 1
                }
                for row in &t.grid {
                    for slot in row {
                        if let CellSlot::Origin(cell) = slot {
                            walk_blocks(&cell.blocks, c);
                        }
                    }
                }
            }
            Block::BlockQuote(inner) => walk_blocks(inner, c),
            _ => {}
        }
    }
}

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap();
        let doc = anydoc::to_document(&bytes, None).unwrap();
        let mut c = Counts::default();
        walk_blocks(&doc.blocks, &mut c);
        for note in &doc.notes {
            walk_blocks(&note.blocks, &mut c);
        }
        let name = path.rsplit('/').next().unwrap_or(&path);
        println!("{name}");
        println!(
            "  runs {} chars {} | bold {} italic {} underline {} strike {} | size {} color {} highlight {} vert {} caps {} | tables {} with_widths {}",
            c.runs,
            c.chars,
            c.bold,
            c.italic,
            c.underline,
            c.strike,
            c.size,
            c.color,
            c.highlight,
            c.vert,
            c.caps,
            c.tables,
            c.with_widths
        );
    }
}
