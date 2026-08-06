fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let doc = anydoc::to_document(&bytes, None).unwrap();
    println!("comments: {}", doc.comments.len());
    for c in doc.comments.iter().take(3) {
        println!("  [{}] {:?} blocks={}", c.id, c.author, c.blocks.len());
    }
}
