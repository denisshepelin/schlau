//! Prints what `schlau_extract::extract` yields for each file given as argument.
//!
//! cargo run --release -p schlau-extract --example extract -- FILE...

use std::path::Path;
use std::time::Instant;

use schlau_extract::{Limits, extract, kind};

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn report(path: &Path, limits: &Limits) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let name = path.file_name().and_then(|n| n.to_str());
    let mime = mime_for(path);
    let started = Instant::now();
    let result = extract(name, mime, &bytes, limits);
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    println!("== {} ({} bytes)", path.display(), bytes.len());
    println!("kind: {:?}", kind(name, mime));
    match result {
        Ok(extracted) => {
            match &extracted.text {
                Some(text) => {
                    let preview: String = text.chars().take(200).collect();
                    println!("text: {} chars: {:?}", text.chars().count(), preview);
                }
                None => println!("text: none"),
            }
            let sizes: Vec<String> = extracted
                .images
                .iter()
                .map(|i| format!("{}x{} {} {}B", i.width, i.height, i.mime, i.bytes.len()))
                .collect();
            println!("images: {} [{}]", extracted.images.len(), sizes.join(", "));
        }
        Err(err) => println!("error: {err}"),
    }
    println!("elapsed: {elapsed:.1} ms");
    Ok(())
}

fn main() -> std::io::Result<()> {
    let limits = Limits::default();
    std::env::args_os()
        .skip(1)
        .try_for_each(|arg| report(Path::new(&arg), &limits))
}
