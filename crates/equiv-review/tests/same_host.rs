//! Same-host float tier (monty replay, 2026-11-10; reopens DECISIONS #17 as an
//! opt-in). By default a float function that calls something equiv cannot
//! analyse (a compiled or sandboxed twin, a libm function) is refused, because
//! its result may differ on another machine. With `same_host`, it is judged on
//! this machine only: bound into the receipt, tier `tested-same-host`, and
//! said in the provenance, so it can never pass as a cross-host verdict.
//! Written before the change (holdout).
use equiv_review::{provenance_note, review, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_sh{}_{name}", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(args: Vec<ArgType>, same_host: bool) -> ReviewSpec {
    ReviewSpec { func: "f".into(), args, n: 300, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host, hinted: false }
}

// An opaque helper (stands in for a compiled or sandboxed twin).
const OPAQUE: &str = "def _h(x):\n    return x * 2.0\n\ndef f(x):\n    return _h(x)\n";
const PLAIN: &str = "def f(x):\n    return x + x\n";

#[test]
fn default_still_refuses_an_opaque_float_callee() {
    let c = tmp("o_c.py", OPAQUE);
    let r = tmp("o_r.py", PLAIN);
    let rec = review(&c, &r, &spec(vec![ArgType::Float], false)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Refused { .. }), "{:?}", rec.verdict);
    assert_eq!(rec.tier, "tested");
}

#[test]
fn same_host_judges_an_opaque_float_callee() {
    let c = tmp("o2_c.py", OPAQUE);
    let r = tmp("o2_r.py", PLAIN);
    let rec = review(&c, &r, &spec(vec![ArgType::Float], true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
    assert_eq!(rec.tier, "tested-same-host");
    assert!(provenance_note(&rec).contains("same-host"), "{}", provenance_note(&rec));
}

#[test]
fn same_host_judges_libm_and_finds_a_float_difference() {
    // round(x, 3) vs a hand-rolled version that differs on some inputs.
    let c = tmp("l_c.py", "import math\n\ndef f(x, n):\n    return round(x, n) + 0.0 * math.sin(0.0)\n");
    let r = tmp("l_r.py", "def f(x, n):\n    return float(int(x * 10 ** n)) / 10 ** n if abs(x) < 1e15 else x\n");
    // n stays small (10 ** n with a huge n would just be slow, not a test).
    let ndig = |same_host| {
        let mut s = spec(vec![ArgType::Float, ArgType::Int], same_host);
        s.domains = vec![equiv_review::Domain::Any, equiv_review::Domain::IntRange(0, 6)];
        s
    };
    let plain = review(&c, &r, &ndig(false)).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Refused { .. }), "{:?}", plain.verdict);
    let rec = review(&c, &r, &ndig(true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "{:?}", rec.verdict);
    assert_eq!(rec.tier, "tested-same-host");
}

#[test]
fn same_host_is_bound_into_the_receipt() {
    let c = tmp("b_c.py", PLAIN);
    let r = tmp("b_r.py", "def f(x):\n    return 2.0 * x\n");
    let a = review(&c, &r, &spec(vec![ArgType::Float], false)).expect("no safety stop");
    let b = review(&c, &r, &spec(vec![ArgType::Float], true)).expect("no safety stop");
    assert_ne!(a.spec_sha256, b.spec_sha256);
    assert_ne!(a.sha256(), b.sha256());
    assert_eq!(a.tier, "tested");
    assert_eq!(b.tier, "tested-same-host");
}

#[test]
fn same_host_lifts_only_the_cross_host_float_refusal() {
    // A non-JSON-structural return is refused for another reason; same-host
    // does not touch it.
    let c = tmp("s_c.py", "def f(x):\n    return {x}\n");
    let r = tmp("s_r.py", "def f(x):\n    return {x}\n");
    let rec = review(&c, &r, &spec(vec![ArgType::Float], true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Refused { .. }), "{:?}", rec.verdict);
}
