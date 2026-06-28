//! Boundary + source-literal input generation: divergences the old seeded
//! random generator could not reach (magic constants, strings longer than 8,
//! uppercase, lists longer than 6) are now caught. These are the missing
//! inputs that used to pass green. (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1 }
}

// A divergence that lives only at a magic constant (x == 777). Uniform random
// over +/-1e6 hits it with ~negligible probability; the literal scan reads 777
// out of the source and tests it directly.
#[test]
fn literal_constant_divergence_is_caught() {
    let cand = tmp("gen_lit_cand.py", "def f(x):\n    return 1 if x == 777 else 0\n");
    let refr = tmp("gen_lit_ref.py", "def f(x):\n    return 0\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int]));
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(
            input.contains("777"),
            "expected the magic constant as the counterexample, got {input}"
        ),
        v => panic!("literal-derived input should catch this, got {v:?}"),
    }
}

// Diverges only for strings longer than 8. The random generator caps length at
// 8, so it can never produce a counterexample; the boundary case (len 10) does.
#[test]
fn long_string_divergence_is_caught() {
    let cand = tmp("gen_long_cand.py", "def f(s):\n    return len(s)\n");
    let refr = tmp("gen_long_ref.py", "def f(s):\n    return min(len(s), 8)\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str]));
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "a >8-char boundary string should catch this, got {:?}",
        r.verdict
    );
}

// Diverges on any uppercase input. The random generator only emits a-z, so it
// never sees an uppercase letter; the boundary case "A" does.
#[test]
fn uppercase_divergence_is_caught() {
    let cand = tmp("gen_upper_cand.py", "def f(s):\n    return s.lower()\n");
    let refr = tmp("gen_upper_ref.py", "def f(s):\n    return s\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str]));
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "an uppercase boundary string should catch this, got {:?}",
        r.verdict
    );
}

// Diverges only when the list is longer than 6. Random lists are <=6 long, so
// they miss it; the boundary case (length 10) catches it.
#[test]
fn long_list_divergence_is_caught() {
    let cand = tmp("gen_llist_cand.py", "def f(xs):\n    return sum(xs)\n");
    let refr = tmp("gen_llist_ref.py", "def f(xs):\n    return sum(xs[:6])\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::ListInt]));
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "a >6-element boundary list should catch this, got {:?}",
        r.verdict
    );
}

// A genuinely behaviour-preserving refactor must still pass: the new corner
// inputs must not manufacture false counterexamples.
#[test]
fn equivalent_refactor_still_passes() {
    let cand = tmp("gen_eq_cand.py", "def f(n):\n    return n * 2\n");
    let refr = tmp("gen_eq_ref.py", "def f(n):\n    return n + n\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int]));
    assert!(
        matches!(r.verdict, ReviewVerdict::Equivalent { .. }),
        "equivalent refactor must stay green, got {:?}",
        r.verdict
    );
}

// AC-1: adding deterministic corner + literal cases must keep receipts
// byte-identical run to run.
#[test]
fn determinism_preserved_with_enumeration() {
    let cand = tmp("gen_det_cand.py", "def f(x):\n    return 1 if x == 5 else x\n");
    let refr = tmp("gen_det_ref.py", "def f(x):\n    return x\n");
    let s = spec("f", vec![ArgType::Int]);
    let r1 = review(&cand, &refr, &s);
    let r2 = review(&cand, &refr, &s);
    assert_eq!(r1.to_bytes(), r2.to_bytes(), "receipts must be reproducible");
    assert_eq!(r1.sha256(), r2.sha256());
}

// Magnitude safety: an O(n) reference must not hang on the enumerated inputs.
// Boundary/literal ints stay inside the envelope, so range(abs(n)) is bounded.
#[test]
fn linear_reference_does_not_hang() {
    let cand = tmp("gen_lin_cand.py", "def f(n):\n    return abs(n)\n");
    let refr = tmp(
        "gen_lin_ref.py",
        "def f(n):\n    c = 0\n    for _ in range(abs(n)): c += 1\n    return c\n",
    );
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int]));
    // Equivalent (both count |n|); the point is it completes, not the verdict.
    assert!(
        matches!(r.verdict, ReviewVerdict::Equivalent { .. }),
        "got {:?}",
        r.verdict
    );
}
