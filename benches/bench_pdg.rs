//! PDG build throughput benchmark.
//!
//! Exists to satisfy the `[[bench]] name = "bench_pdg"` target declared in
//! Cargo.toml. Measures CFG construction and PDG build over a set of C#
//! sources given on the command line (or a small built-in sample when none are
//! provided), so `cargo bench --bench bench_pdg` works out of the box.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tiny_pdg_cs::cfg::builder::build_cfg;
use tiny_pdg_cs::pdg::pdg_builder::build_pdg;

/// Collect .cs files recursively.
fn collect_cs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_cs_files(&path, files);
        } else if path.extension().map_or(false, |e| e == "cs") {
            files.push(path);
        }
    }
}

const SAMPLE: &str = r#"
public class Sample
{
    public int Classify(int n)
    {
        if (n < 0)
        {
            return -1;
        }
        else if (n == 0)
        {
            return 0;
        }
        var scaled = n * 2;
        var total = 0;
        for (var i = 0; i < scaled; i++)
        {
            if (i % 3 == 0)
            {
                total += i;
            }
            else
            {
                total -= i;
            }
        }
        return total;
    }
}
"#;

fn load_sources(paths: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    for p in paths {
        collect_cs_files(Path::new(p), &mut files);
    }
    let mut sources: Vec<String> = files
        .iter()
        .filter_map(|f| fs::read_to_string(f).ok())
        .collect();
    if sources.is_empty() {
        sources.push(SAMPLE.to_string());
    }
    sources
}

fn bench_build_cfg(c: &mut Criterion) {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let sources = load_sources(&paths);

    c.bench_function("cfg_build", |b| {
        b.iter(|| {
            let mut blocks = 0usize;
            for src in &sources {
                if let Ok(cfg) = build_cfg(src) {
                    blocks += cfg.node_count();
                }
            }
            black_box(blocks)
        });
    });
}

fn bench_build_pdg(c: &mut Criterion) {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let sources = load_sources(&paths);

    c.bench_function("pdg_build", |b| {
        b.iter(|| {
            let mut nodes = 0usize;
            let mut edges = 0usize;
            for src in &sources {
                if let Ok(cfg) = build_cfg(src) {
                    if let Ok(pdg) = build_pdg(&cfg) {
                        nodes += pdg.node_count();
                        edges += pdg.edge_count();
                    }
                }
            }
            black_box((nodes, edges))
        });
    });
}

fn bench_end_to_end(c: &mut Criterion) {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let sources = load_sources(&paths);

    c.bench_function("cfg_plus_pdg", |b| {
        b.iter(|| {
            let start = Instant::now();
            let mut total = 0usize;
            for src in &sources {
                if let Ok(cfg) = build_cfg(src) {
                    if let Ok(pdg) = build_pdg(&cfg) {
                        total += pdg.node_count();
                    }
                }
            }
            black_box((total, start.elapsed().as_nanos()))
        });
    });
}

criterion_group!(benches, bench_build_cfg, bench_build_pdg, bench_end_to_end);
criterion_main!(benches);
