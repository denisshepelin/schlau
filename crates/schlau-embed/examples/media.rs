//! Ranks images against text queries and reports latency, with the built-in model or a server.
//!
//! Run with: cargo run --release -p schlau-embed --features metal --example media -- <model dir> <image>... -- <query>...
//! A server instead: SCHLAU_EMBED_URL=http://127.0.0.1:8080 cargo run … (same arguments).
//! With both, SCHLAU_EMBED_COMPARE=1 also prints how close the server's vectors are to the built-in model's.

use std::sync::Arc;
use std::time::Instant;

use schlau_embed::{GemmaEmbedder, MediaEmbedder, MediaInput, RemoteEmbedder, Towers, vector::dot};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().position(|a| a == "--").ok_or("usage: media <model dir> <image>... -- <query>...")?;
    let (dir, images, queries) = (&args[0], &args[1..split], &args[split + 1..]);
    let start = Instant::now();
    let embedder: Box<dyn MediaEmbedder> = match std::env::var("SCHLAU_EMBED_URL") {
        Ok(url) => Box::new(RemoteEmbedder::connect(&url, &std::env::var("SCHLAU_EMBED_MODEL").unwrap_or("embeddinggemma-2".into()), None)?),
        Err(_) => Box::new(GemmaEmbedder::load(dir.as_ref(), Towers::TextAndVision)?),
    };
    println!("loaded {} in {:.0} ms", embedder.id(), ms(start));
    let inputs: Vec<MediaInput> = images
        .iter()
        .map(|p| std::fs::read(p).map(|b| MediaInput::Image { bytes: Arc::from(b), mime: "image/jpeg".into() }))
        .collect::<Result<_, _>>()?;
    embedder.embed(&inputs[..1])?;
    let start = Instant::now();
    let vectors = embedder.embed(&inputs)?;
    println!("{} images in {:.0} ms ({:.0} ms each)", inputs.len(), ms(start), ms(start) / inputs.len() as f64);
    if std::env::var("SCHLAU_EMBED_COMPARE").is_ok() {
        let local = GemmaEmbedder::load(dir.as_ref(), Towers::TextAndVision)?;
        let reference = local.embed(&inputs)?;
        images.iter().zip(reference.iter().zip(&vectors)).for_each(|(p, (a, b))| println!("server vs built-in, {p}: cosine {:.4}", dot(a, b)));
        let (a, b) = (local.embed_query("a dog on the beach")?, embedder.embed_query("a dog on the beach")?);
        println!("server vs built-in, query: cosine {:.4}", dot(&a, &b));
    }
    embedder.embed_query("warm up")?;
    for q in queries {
        let start = Instant::now();
        let v = embedder.embed_query(q)?;
        let took = ms(start);
        let mut ranked: Vec<(f32, &String)> = vectors.iter().map(|d| dot(&v, d)).zip(images).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("{q:?} ({took:.1} ms): {}", ranked.iter().take(3).map(|(s, p)| format!("{} {s:.3}", p.rsplit('/').next().unwrap_or(p))).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
