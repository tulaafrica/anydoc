//! End-to-end FFI check, runnable on host and on the phone: converts each
//! file through the SAME buffer contract the app will use, validates the
//! layout, and times it.
fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let t = std::time::Instant::now();
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let code = unsafe {
            anydoc_mobile::anydoc_tula_convert(bytes.as_ptr(), bytes.len(), &mut out, &mut out_len)
        };
        let ms = t.elapsed().as_secs_f64() * 1e3;
        assert_eq!(code, 0);
        let buffer = unsafe { std::slice::from_raw_parts(out, out_len) };
        let json_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&buffer[4..4 + json_len]).unwrap();
        let blob_len = out_len - 4 - json_len;
        let status = if json.contains("\"status\":\"ok\"") { "ok      " } else { "fallback" };
        println!(
            "{status} {ms:7.1}ms  json {:6}kB  assets {:6}kB  {name}",
            json_len / 1024,
            blob_len / 1024
        );
        if status.trim() == "fallback" {
            println!("         {}", &json[..json.len().min(120)]);
        }
        // ANYDOC_DUMP=1: emit the whole IR JSON, for host-side fidelity diffs.
        if std::env::var("ANYDOC_DUMP").is_ok() {
            println!("{json}");
        }
        unsafe { anydoc_mobile::anydoc_tula_free(out, out_len) };
    }
}
