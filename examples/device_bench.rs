//! On-device conversion benchmark. Pushed to /data/local/tmp on a real phone
//! and run over real documents: the number that matters for Tula is ms on a
//! Galaxy A05s, not ms on an M-series Mac.
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{:<38} {:>9} {:>12} {:>12}", "file", "kB", "toMarkdown", "toDocument");
    for path in &args {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        let name = path.rsplit('/').next().unwrap_or(path);
        // Warm once (page cache, allocator), then median of 9.
        let _ = anydoc::to_markdown_bytes(&bytes, None);
        let mut md_ms = vec![];
        let mut doc_ms = vec![];
        for _ in 0..9 {
            let t = Instant::now();
            let md = anydoc::to_markdown_bytes(&bytes, None);
            md_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let doc = anydoc::to_document(&bytes, None);
            doc_ms.push(t.elapsed().as_secs_f64() * 1e3);
            if let (Err(e), Err(_)) = (&md, &doc) {
                println!("{name}: {e:?}");
                break;
            }
        }
        md_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        doc_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = |v: &Vec<f64>| if v.is_empty() { f64::NAN } else { v[v.len() / 2] };
        println!(
            "{:<38} {:>9.0} {:>10.1}ms {:>10.1}ms",
            &name[..name.len().min(38)],
            bytes.len() as f64 / 1024.0,
            med(&md_ms),
            med(&doc_ms)
        );
    }
}
