//! A Rust panic surfacing through PyO3 (pyo3_runtime.PanicException, a
//! BaseException) is an outcome of the function, not a reason to stop
//! (path A step 2 gap: pendulum week_day). Other BaseExceptions still stop.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_panic_{}_{name}.py", std::process::id()));
    std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
    p
}

fn spec() -> ReviewSpec {
    ReviewSpec { func: "f".into(), args: vec![ArgType::Int], n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false }
}

const PANIC: &str = "class PanicException(BaseException):\n    __module__ = 'pyo3_runtime'\n";

#[test]
fn a_panic_on_one_side_is_a_counterexample() {
    let c = tmp("c", &format!("{PANIC}def f(x):\n    if x < 0:\n        raise PanicException('index out of bounds')\n    return x\n"));
    let r = tmp("r", "def f(x):\n    return x\n");
    match review(&c, &r, &spec()).expect("no safety stop").verdict {
        ReviewVerdict::Counterexample { candidate, reference, .. } => {
            assert!(candidate.contains("PanicException"), "{candidate}");
            assert!(!reference.contains("PanicException"), "{reference}");
        }
        v => panic!("expected a counterexample, got {v:?}"),
    }
}

#[test]
fn the_same_panic_on_both_sides_is_agreement() {
    let body = format!("{PANIC}def f(x):\n    if x < 0:\n        raise PanicException('boom')\n    return x * 2\n");
    let c = tmp("bc", &body);
    let r = tmp("br", &body);
    let v = review(&c, &r, &spec()).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

#[test]
fn other_base_exceptions_still_stop_the_run() {
    let c = tmp("sc", "def f(x):\n    if x < 0:\n        raise SystemExit(3)\n    return x\n");
    let r = tmp("sr", "def f(x):\n    return x\n");
    let v = review(&c, &r, &spec()).expect("no safety stop").verdict;
    assert!(
        !matches!(v, ReviewVerdict::Counterexample { .. } | ReviewVerdict::Equivalent { .. }),
        "a SystemExit must not be judged, got {v:?}"
    );
}
