//! Type-admissibility constraint tests for float64 (spec-type-admissibility.md
//! AD-1, AD-2, AD-4). Written before the implementation, per the ContextKey
//! lesson: a float design that cannot pass these is the wrong design.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn write_py(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_ftest_{name}.py"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1 }
}

// AD-1: an admissible float refactor (only the correctly rounded basic ops)
// is judged Equivalent. The receipt is byte-identical run to run
// (cross-host reproducibility is anchored on this determinism + exact bits).
#[test]
fn ad1_admissible_float_refactor_equivalent_and_reproducible() {
    let cand = write_py("ad1_c", "def scale(x):\n    return x * 2.0 + 1.0\n");
    let refr = write_py("ad1_r", "def scale(x):\n    return x + x + 1.0\n");
    let s = spec("scale", vec![ArgType::Float]);
    let r1 = review(&cand, &refr, &s);
    let r2 = review(&cand, &refr, &s);
    assert!(matches!(r1.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r1.verdict);
    assert_eq!(r1.sha256(), r2.sha256(), "receipt must be byte-identical across runs");
}

// AD-1: a real float divergence yields a stable counterexample (canonical
// bits), reproducible to identical receipt bytes.
#[test]
fn ad1_float_divergence_has_stable_counterexample() {
    let cand = write_py("ad1d_c", "def f(x):\n    return x * 2.0\n");
    let refr = write_py("ad1d_r", "def f(x):\n    return x * 3.0\n");
    let s = spec("f", vec![ArgType::Float]);
    let r1 = review(&cand, &refr, &s);
    let r2 = review(&cand, &refr, &s);
    assert!(matches!(r1.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r1.verdict);
    assert_eq!(r1.sha256(), r2.sha256());
}

// Signed zero is observable (1/+0 = +inf, 1/-0 = -inf). So +0.0 and -0.0
// must be treated as DIFFERENT behaviour.
#[test]
fn signed_zero_is_distinguished() {
    let cand = write_py("sz_c", "def z(x):\n    return 0.0\n");
    let refr = write_py("sz_r", "def z(x):\n    return -0.0\n");
    let s = spec("z", vec![ArgType::Float]);
    let r = review(&cand, &refr, &s);
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
}

// NaN payloads are not observable behaviour: any NaN equals any NaN.
#[test]
fn nan_equals_nan() {
    let cand = write_py("nan_c", "def n(x):\n    return float('nan')\n");
    let refr = write_py("nan_r", "def n(x):\n    return float('nan') * x\n");
    let s = spec("n", vec![ArgType::Float]);
    let r = review(&cand, &refr, &s);
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// AD-2: a function that reaches a transcendental (not correctly-rounded by
// IEEE-754) is REFUSED by name, never Equivalent or Counterexample.
#[test]
fn ad2_transcendental_is_refused() {
    let cand = write_py("tr_c", "import math\ndef g(x):\n    return math.sin(x)\n");
    let refr = write_py("tr_r", "from math import sin\ndef g(x):\n    return sin(x)\n");
    let s = spec("g", vec![ArgType::Float]);
    let r = review(&cand, &refr, &s);
    match r.verdict {
        ReviewVerdict::Refused { ref reason } => assert!(reason.contains("sin"), "reason: {reason}"),
        other => panic!("expected Refused, got {other:?}"),
    }
    assert_ne!(r.verdict.exit_code(), 0, "refused must not pass the gate");
}
