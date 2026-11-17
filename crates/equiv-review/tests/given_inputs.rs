//! Given inputs (DECISIONS #39): the caller supplies every input as typed JSON
//! lines. Properties:
//! - exactly the given inputs are judged, in order, nothing generated;
//! - bytes and nested values (list, tuple, dict with non-str keys, None) reach
//!   both sides exactly;
//! - the file is bound into the spec hash: same file, same receipt id; another
//!   file, another id; a generated review over the same function differs too;
//! - the receipt counts the inputs as given.

use equiv_review::{given, review_given, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn wp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_given_{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}
fn spec() -> ReviewSpec {
    ReviewSpec { func: "f".into(), args: vec![], n: 0, seed: 0, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false, hinted: false }
}
// A tree-like map: name bytes -> (mode, id bytes), as a caller would draw it.
fn line(names: &[&str]) -> String {
    let pairs: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let hex: String = n.bytes().map(|b| format!("{b:02x}")).collect();
            format!(r#"[{{"bytes":"{hex}"}},{{"tuple":[{{"int":"{}"}},{{"bytes":"00ff"}}]}}]"#, if i % 2 == 0 { 16384 } else { 33188 })
        })
        .collect();
    format!("[{{\"dict\":[{}]}},{{\"none\":null}}]\n", pairs.join(","))
}
const REF: &str = "def f(entries, _unused):\n    return [(k, m, s) for k, (m, s) in sorted(entries.items())]\n";
// Differs only when some name contains b'/': sorts those last.
const CAND: &str = "def f(entries, _unused):\n    items = sorted(entries.items(), key=lambda kv: (b'/' in kv[0], kv[0]))\n    return [(k, m, s) for k, (m, s) in items]\n";

#[test]
fn exactly_the_given_inputs_are_judged() {
    let text = [line(&["b", "a"]), line(&["x", "y", "z"]), line(&["a/", "b"])].concat();
    let g = given::parse(text.as_bytes()).unwrap();
    let (c, r) = (wp("ok_c", CAND), wp("ok_r", REF));
    let rec = review_given(&c, &r, &spec(), &g, 120, None).ok().expect("a receipt");
    match &rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(input.contains("b'a/'"), "{input}"),
        v => panic!("expected a counterexample, got {v:?}"),
    }
    assert_eq!(rec.inputs.given, 3);
    assert_eq!((rec.inputs.corner, rec.inputs.literal, rec.inputs.random, rec.inputs.harvested), (0, 0, 0, 0));

    // Without the input that tells them apart: equivalent on what was given.
    let g2 = given::parse([line(&["b", "a"]), line(&["x", "y", "z"])].concat().as_bytes()).unwrap();
    let rec2 = review_given(&c, &r, &spec(), &g2, 120, None).ok().expect("a receipt");
    assert!(matches!(rec2.verdict, ReviewVerdict::Equivalent { n: 2, .. }), "{:?}", rec2.verdict);
}

#[test]
fn the_file_is_bound_into_the_receipt() {
    let (c, r) = (wp("id_c", REF), wp("id_r", REF));
    let a = given::parse(line(&["a", "b"]).as_bytes()).unwrap();
    let b = given::parse(line(&["a", "c"]).as_bytes()).unwrap();
    let ra1 = review_given(&c, &r, &spec(), &a, 120, None).ok().unwrap();
    let ra2 = review_given(&c, &r, &spec(), &a, 120, None).ok().unwrap();
    let rb = review_given(&c, &r, &spec(), &b, 120, None).ok().unwrap();
    assert_eq!(ra1.sha256(), ra2.sha256());
    assert_ne!(ra1.spec_sha256, rb.spec_sha256);
    assert_ne!(ra1.sha256(), rb.sha256());
}

#[test]
fn a_rust_style_panic_on_a_given_input_is_an_outcome() {
    // pyo3's PanicException derives from BaseException; equiv observes it by name.
    let c = wp("pa_c", "class PanicException(BaseException): pass\ndef f(entries, _unused):\n    if len(entries) > 2: raise PanicException('order')\n    return sorted(entries)\n");
    let r = wp("pa_r", "def f(entries, _unused):\n    return sorted(entries)\n");
    let g = given::parse([line(&["a", "b"]), line(&["a", "b", "c"])].concat().as_bytes()).unwrap();
    let rec = review_given(&c, &r, &spec(), &g, 120, None).ok().unwrap();
    match &rec.verdict {
        ReviewVerdict::Counterexample { candidate, .. } => assert!(candidate.contains("PanicException"), "{candidate}"),
        v => panic!("expected a counterexample, got {v:?}"),
    }
}
