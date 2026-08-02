//! How much of a conversion is ingest? Run against the corpus:
//!     cargo run --release --example ingest_share
use std::time::Instant;

fn main() {
    let dir = std::path::Path::new("corpus");
    let mut papers: Vec<_> = std::fs::read_dir(dir)
        .expect("corpus/ not found")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    papers.sort();

    let (mut ingest, mut total) = (0.0f64, 0.0f64);
    for p in &papers {
        let t = Instant::now();
        let raw = rustypaper::extract(p).expect("extract");
        let i = t.elapsed().as_secs_f64();
        std::hint::black_box(&raw);

        let t = Instant::now();
        let doc = rustypaper::convert(p).expect("convert");
        let c = t.elapsed().as_secs_f64();
        std::hint::black_box(&doc);

        println!(
            "{:<18} ingest {:.3}s  total {:.3}s  {:.0}%",
            p.file_name().unwrap().to_string_lossy(),
            i,
            c,
            100.0 * i / c
        );
        ingest += i;
        total += c;
    }
    println!(
        "\ncorpus: ingest {ingest:.2}s of total {total:.2}s = {:.0}%",
        100.0 * ingest / total
    );
}
