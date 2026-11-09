//! Constraint tests for honest "equivalent" verdicts (written before the change).
//! The founding rule: never claim more than the evidence shows.
//! - If every generated input made both versions raise, only errors were
//!   compared: that is "could not check" (Refused, exit 2), never EQUIVALENT.
//! - If fewer than THIN_K inputs returned a value, the verdict stays equivalent
//!   but is labelled thin evidence, in the receipt (tier "tested-thin") and in
//!   every rendering, including the PR comment's headline.
//! - A function with no arguments, or only a handful of possible inputs, is
//!   checked completely by those few calls: exempt from both rules.
//! - A difference in WHICH error is raised is still a counterexample.

use equiv_review::{
    render_markdown, review, thin_evidence, ArgType, ReviewItem, ReviewSpec, ReviewVerdict, THIN_K,
};
use std::io::Write;

fn wp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_honest_{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}
fn spec(args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: "f".into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false }
}

const ERRORS_ONLY: &str = "def f(s):\n    raise ValueError(s)\n";
// Only the literal '#fff' (scanned from the source) reaches a returned value.
const THIN: &str = "def f(s):\n    if s == '#fff':\n        return 1\n    raise ValueError(s)\n";

#[test]
fn errors_only_is_could_not_check_never_equivalent() {
    let p = wp("errs", ERRORS_ONLY);
    let r = review(&p, &p, &spec(vec![ArgType::Str])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Refused { reason } => {
            assert!(reason.contains("only errors were compared"), "reason: {reason}")
        }
        v => panic!("errors-only must not be equivalent, got {v:?}"),
    }
    assert_eq!(r.verdict.exit_code(), 2);
}

#[test]
fn different_error_is_still_a_counterexample() {
    let c = wp("de_c", "def f(s):\n    raise ValueError(s)\n");
    let b = wp("de_r", "def f(s):\n    raise TypeError(s)\n");
    let r = review(&c, &b, &spec(vec![ArgType::Str])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
}

#[test]
fn thin_equivalent_is_labelled_in_the_receipt() {
    let p = wp("thin", THIN);
    let r1 = review(&p, &p, &spec(vec![ArgType::Str])).expect("no safety stop");
    let r2 = review(&p, &p, &spec(vec![ArgType::Str])).expect("no safety stop");
    let n = match r1.verdict {
        ReviewVerdict::Equivalent { n, .. } => n,
        ref v => panic!("expected equivalent, got {v:?}"),
    };
    let v = thin_evidence(n, r1.exceptions).expect("must be thin");
    assert!(v >= 1 && v < THIN_K, "value-returning inputs: {v}");
    assert_eq!(r1.tier, "tested-thin");
    assert_eq!(r1.sha256(), r2.sha256(), "receipt reproducible");
}

#[test]
fn thin_shows_in_the_pr_comment_headline_and_row() {
    let p = wp("thin_md", THIN);
    let r = review(&p, &p, &spec(vec![ArgType::Str])).expect("no safety stop");
    let md = render_markdown(&[ReviewItem { name: "f".into(), outcome: Ok(r) }], None);
    let headline = md.lines().find(|l| l.contains("checked")).unwrap_or("");
    assert!(headline.contains("thin evidence"), "headline must not read as a plain pass:\n{md}");
    assert!(md.contains("| `f` | equivalent (thin evidence) |"), "row must carry the label:\n{md}");
}

#[test]
fn ordinary_equivalent_is_unchanged() {
    let p = wp("plain", "def f(n):\n    return n * 2\n");
    let r = review(&p, &p, &spec(vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    assert_eq!(r.tier, "tested");
    let md = render_markdown(&[ReviewItem { name: "f".into(), outcome: Ok(r) }], None);
    assert!(!md.contains("thin evidence"), "{md}");
}

#[test]
fn no_argument_function_is_exempt_even_when_it_raises() {
    let p = wp("zero", "def f():\n    raise ValueError('x')\n");
    let r = review(&p, &p, &spec(vec![])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { n: 1, .. }), "one call is a complete check: {:?}", r.verdict);
    assert_eq!(r.tier, "tested");
}

#[test]
fn small_input_space_is_exempt() {
    // Two possible inputs: both raise on both sides. Checked completely, so
    // neither "only errors" nor "thin" applies.
    let p = wp("bool", "def f(b):\n    raise ValueError(b)\n");
    let r = review(&p, &p, &spec(vec![ArgType::Bool])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    assert_eq!(r.tier, "tested");
}
