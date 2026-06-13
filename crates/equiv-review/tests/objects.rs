//! Type-admissibility constraint tests for JSON-structural objects
//! (spec-type-admissibility.md §5). Written before the implementation. The
//! property: a dict is a MAP (key order is not observable). A refactor that
//! changes key insertion order is equivalent; anything not JSON-structural
//! (sets, custom objects, non-string keys) is refused, never judged.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn wp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_otest_{name}.py"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}
fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1 }
}

// A dict is a map: key insertion order is not behaviour. Same content in a
// different order is Equivalent. The receipt is reproducible.
#[test]
fn dict_return_key_order_does_not_matter() {
    let cand = wp("ko_c", "def f(n):\n    return {'a': n, 'b': n + 1}\n");
    let refr = wp("ko_r", "def f(n):\n    return {'b': n + 1, 'a': n}\n");
    let s = spec("f", vec![ArgType::Int]);
    let r1 = review(&cand, &refr, &s);
    let r2 = review(&cand, &refr, &s);
    assert!(matches!(r1.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r1.verdict);
    assert_eq!(r1.sha256(), r2.sha256());
}

// Genuinely different map content is caught.
#[test]
fn dict_content_divergence_caught() {
    let cand = wp("dc_c", "def f(n):\n    return {'a': n}\n");
    let refr = wp("dc_r", "def f(n):\n    return {'a': n + 1}\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int]));
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
}

// Canonicalisation recurses: nested dict values are order-independent too.
#[test]
fn nested_dict_value_order_independent() {
    let cand = wp("nd_c", "def f(n):\n    return {'x': {'a': n, 'b': 1}}\n");
    let refr = wp("nd_r", "def f(n):\n    return {'x': {'b': 1, 'a': n}}\n");
    assert!(matches!(review(&cand, &refr, &spec("f", vec![ArgType::Int])).verdict, ReviewVerdict::Equivalent { .. }));
}

// A set is not JSON-structural (no stable cross-host form): refuse, never judge.
#[test]
fn set_return_is_refused() {
    let p = wp("set", "def f(n):\n    return {n, n + 1}\n");
    assert!(matches!(review(&p, &p, &spec("f", vec![ArgType::Int])).verdict, ReviewVerdict::Refused { .. }));
}

// Non-string dict keys cannot be canonicalised as JSON: refuse.
#[test]
fn non_string_keys_refused() {
    let p = wp("nsk", "def f(n):\n    return {n: 'x'}\n");
    assert!(matches!(review(&p, &p, &spec("f", vec![ArgType::Int])).verdict, ReviewVerdict::Refused { .. }));
}

// The dict ARG type: a function that takes a dict is reviewed normally.
#[test]
fn dict_arg_function_reviewed() {
    let cand = wp("da_c", "def total(d):\n    return sum(d.values())\n");
    let refr = wp("da_r", "def total(d):\n    return sum(v for v in d.values())\n");
    let r = review(&cand, &refr, &spec("total", vec![ArgType::Dict]));
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// Cross-host regression guard: a fixed structural-dict counterexample review
// must always produce this exact receipt-id (the CI matrix runs it on all
// three targets, proving the canonical form is byte-identical cross-host).
#[test]
fn golden_dict_receipt_id_is_stable() {
    let cand = wp("gd_c", "def f(n):\n    return {'a': n}\n");
    let refr = wp("gd_r", "def f(n):\n    return {'a': n + 1}\n");
    let id: String = review(&cand, &refr, &spec("f", vec![ArgType::Int])).sha256().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(id, "1513c2ab6b5747d25d414303d404591f78c44c76496adcd702d6d1b359af3065", "receipt-id drifted");
}
