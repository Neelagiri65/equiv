//! Inputs harvested from the project's own calls and tests (0.17.0).
//! Constraints pinned here:
//! - literal arguments of calls (positional and keyword) and pytest
//!   parametrize rows are collected; non-literals and wrong types are not;
//! - harvested inputs are bound into the receipt (spec hash and count), and a
//!   review with none is byte-identical to one without the feature;
//! - a counterexample on a harvested input is reported first.
//! (require python3 on PATH)
use equiv_review::harvest::{expand_paths, harvest};
use equiv_review::{review_limited, review_with_inputs, ArgType, PyVal, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::PathBuf;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_harvest_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("tests")).unwrap();
    d
}

const REF: &str = "def norm(s, upper):\n    t = s.strip().replace('-', '')\n    return t.upper() if upper else t\n";
const CAND: &str = "def norm(s, upper):\n    t = s.replace(' ', '').replace('-', '')\n    return t.upper() if upper else t\n";
const TESTS: &str = r#"import pytest
from m import norm

def test_calls(x):
    assert norm("ab-12", True) == "AB12"
    assert norm(s="x y", upper=False) == "xy"
    assert norm(x, True)            # non-literal first argument
    assert norm(42, True)           # wrong type: not harvested
    assert obj.norm("k-1", False)   # attribute call

@pytest.mark.parametrize("raw, expected", [("GB 29", "GB29"), ("de-44", "DE44")])
def test_rows(raw, expected):
    assert norm(raw, True) == expected
"#;

fn spec() -> ReviewSpec {
    ReviewSpec { func: "norm".into(), args: vec![ArgType::Str, ArgType::Bool], n: 200, seed: 1, budget: DEFAULT_BUDGET }
}

#[test]
fn collects_calls_keywords_and_parametrize_rows() {
    let d = dir("collect");
    std::fs::write(d.join("cand.py"), CAND).unwrap();
    std::fs::write(d.join("tests/test_m.py"), TESTS).unwrap();
    let files = expand_paths(&[d.join("tests")]);
    let h = harvest(&[("norm".into(), d.join("cand.py"), vec![ArgType::Str, ArgType::Bool])], &files).pop().unwrap();
    let s = |x: &str| Some(PyVal::Str(x.into()));
    let b = |x: bool| Some(PyVal::Bool(x));
    // Parametrized rows come first (the test function is visited first),
    // then the remaining calls in file order.
    for want in [
        vec![s("GB 29"), b(true)],
        vec![s("de-44"), b(true)],
        vec![s("ab-12"), b(true)],
        vec![s("x y"), b(false)],
        vec![None, b(true)],
        vec![s("k-1"), b(false)],
    ] {
        assert!(h.contains(&want), "missing {want:?} in {h:?}");
    }
    assert!(!h.iter().any(|row| row[0] == Some(PyVal::Int(42))), "a wrong-typed literal was kept: {h:?}");
}

#[test]
fn harvested_inputs_are_bound_into_the_receipt_and_reported_first() {
    let d = dir("bind");
    std::fs::write(d.join("cand.py"), CAND).unwrap();
    std::fs::write(d.join("ref.py"), REF).unwrap();
    std::fs::write(d.join("tests/test_m.py"), TESTS).unwrap();
    let files = expand_paths(&[d.join("tests")]);
    let h = harvest(&[("norm".into(), d.join("cand.py"), vec![ArgType::Str, ArgType::Bool])], &files).pop().unwrap();

    let plain = review_limited(&d.join("cand.py"), &d.join("ref.py"), &spec(), 60).unwrap();
    let none = review_with_inputs(&d.join("cand.py"), &d.join("ref.py"), &spec(), &[], 60).unwrap();
    assert_eq!(plain.to_bytes(), none.to_bytes(), "no harvested inputs must mean an identical receipt");
    assert_eq!(plain.inputs.harvested, 0);

    let with = review_with_inputs(&d.join("cand.py"), &d.join("ref.py"), &spec(), &h, 60).unwrap();
    assert!(with.inputs.harvested > 0);
    assert_ne!(with.spec_sha256, plain.spec_sha256, "harvested inputs must change the spec hash");
    match &with.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert_eq!(input, "('GB 29', True)"),
        v => panic!("expected a counterexample, got {v:?}"),
    }
    let again = review_with_inputs(&d.join("cand.py"), &d.join("ref.py"), &spec(), &h, 60).unwrap();
    assert_eq!(with.to_bytes(), again.to_bytes(), "same harvested inputs, same receipt");
}

/// A function with no arguments is called once and compared, not reported
/// as could-not-check (it used to end in a driver SyntaxError).
#[test]
fn zero_argument_functions_are_checked() {
    let d = dir("zeroarg");
    std::fs::write(d.join("a.py"), "def f():\n    return 1\n").unwrap();
    std::fs::write(d.join("b.py"), "def f():\n    return 2\n").unwrap();
    std::fs::write(d.join("c.py"), "def f():\n    return 1\n").unwrap();
    let spec = ReviewSpec { func: "f".into(), args: vec![], n: 200, seed: 1, budget: DEFAULT_BUDGET };
    let diff = review_limited(&d.join("a.py"), &d.join("b.py"), &spec, 60).unwrap();
    assert!(matches!(diff.verdict, ReviewVerdict::Counterexample { .. }), "{:?}", diff.verdict);
    let same = review_limited(&d.join("a.py"), &d.join("c.py"), &spec, 60).unwrap();
    assert!(matches!(same.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", same.verdict);
}

/// A harvested value that runs past the step budget (a project's own test of
/// a brute-force function) is "not judged at this input"; the review still
/// judges every other input instead of ending as could not check.
#[test]
fn a_costly_harvested_value_is_not_judged_rather_than_fatal() {
    let d = dir("costly");
    let src = "def f(s):\n    n = 0\n    if s == 'slow':\n        while True:\n            n += 1\n    return len(s)\n";
    std::fs::write(d.join("a.py"), src).unwrap();
    std::fs::write(d.join("b.py"), src).unwrap();
    let spec = ReviewSpec { func: "f".into(), args: vec![ArgType::Str], n: 200, seed: 1, budget: DEFAULT_BUDGET };
    let slow: Vec<equiv_review::Slots> = vec![vec![Some(PyVal::Str("slow".into()))]];
    let r = review_with_inputs(&d.join("a.py"), &d.join("b.py"), &spec, &slow, 60).unwrap();
    match r.verdict {
        ReviewVerdict::Equivalent { not_judged, .. } => assert_eq!(not_judged, 1),
        v => panic!("expected EQUIVALENT with one input not judged, got {v:?}"),
    }
}
