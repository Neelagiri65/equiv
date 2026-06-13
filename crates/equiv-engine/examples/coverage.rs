//! Measure the REAL BMC accept-rate over the compiled HumanEval corpus.
//! Each module exports its candidate function as `__cand__`.
use equiv_engine::bmc::{coverage_probe, Coverage};
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: coverage <wasm-dir>");
    let mut provable = 0;
    let mut out = 0;
    let mut notfound = 0;
    let mut total = 0;
    let mut examples: Vec<String> = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "wasm"))
        .collect();
    entries.sort();
    for p in &entries {
        let bytes = std::fs::read(p).unwrap();
        total += 1;
        match coverage_probe(&bytes, "__cand__") {
            Coverage::Provable => {
                provable += 1;
                if examples.len() < 12 {
                    examples.push(format!("  PROVABLE: {}", file(p)));
                }
            }
            Coverage::OutOfEnvelope => out += 1,
            Coverage::NotFound => notfound += 1,
        }
    }
    println!("BMC proving-path accept-rate over {} HumanEval WASM modules:", total);
    println!("  Provable (extract+execute OK): {provable}  ({:.1}%)", pct(provable, total));
    println!("  OutOfEnvelope:                 {out}  ({:.1}%)", pct(out, total));
    println!("  NotFound (__cand__ missing):   {notfound}");
    println!("\nProvable examples:");
    for e in &examples { println!("{e}"); }
}
fn file(p: &Path) -> String { p.file_name().unwrap().to_string_lossy().into_owned() }
fn pct(n: usize, d: usize) -> f64 { if d == 0 { 0.0 } else { 100.0 * n as f64 / d as f64 } }
