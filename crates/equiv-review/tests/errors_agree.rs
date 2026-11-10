//! Parity-mode noise (adoption run 2026-11-10): two implementations of the same
//! function often reject bad input with different exception classes (a Rust or
//! Cython twin raises ValueError or OverflowError where Python raises TypeError
//! or a custom error). With `errors_agree`, "both sides raised" counts as
//! agreement whatever the class, so the first counterexample shown is one where
//! the twins really differ. Written before the change (holdout).
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_ea{}_{name}", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>, errors_agree: bool) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 300, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree, domains: vec![], same_host: false }
}

// Same values on valid input; bad input rejected with different classes.
const CAND_CLASS: &str = "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x * 2\n";
const REF_CLASS: &str = "def f(x):\n    if x < 0:\n        raise TypeError('neg')\n    return x + x\n";

#[test]
fn default_still_reports_a_different_exception_class() {
    let c = tmp("cls_c.py", CAND_CLASS);
    let r = tmp("cls_r.py", REF_CLASS);
    let rec = review(&c, &r, &spec("f", vec![ArgType::Int], false)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", rec.verdict);
}

#[test]
fn errors_agree_treats_both_raising_as_agreement() {
    let c = tmp("cls2_c.py", CAND_CLASS);
    let r = tmp("cls2_r.py", REF_CLASS);
    let rec = review(&c, &r, &spec("f", vec![ArgType::Int], true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
}

#[test]
fn errors_agree_still_catches_raise_versus_value() {
    // Python accepts what the twin rejects (the pendulum shape): one side
    // raises, the other returns a value. That is a real difference.
    let c = tmp("rv_c.py", "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x * 2\n");
    let r = tmp("rv_r.py", "def f(x):\n    return x * 2\n");
    let rec = review(&c, &r, &spec("f", vec![ArgType::Int], true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", rec.verdict);
}

#[test]
fn errors_agree_still_catches_a_different_value() {
    let c = tmp("val_c.py", "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x * 2 + (1 if x == 7 else 0)\n");
    let r = tmp("val_r.py", REF_CLASS);
    let rec = review(&c, &r, &spec("f", vec![ArgType::Int], true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", rec.verdict);
}

#[test]
fn errors_agree_changes_the_receipt_and_default_does_not() {
    // The mode is bound into the receipt id, so a pass under errors_agree can
    // never be mistaken for a strict pass. Strict receipts keep their old ids
    // (the goldens in budget/floats/objects/tuples guard that).
    let c = tmp("rid_c.py", "def f(x):\n    return x * 2\n");
    let r = tmp("rid_r.py", "def f(x):\n    return x + x\n");
    let a = review(&c, &r, &spec("f", vec![ArgType::Int], false)).expect("no safety stop");
    let b = review(&c, &r, &spec("f", vec![ArgType::Int], true)).expect("no safety stop");
    assert_ne!(a.sha256(), b.sha256());
}
