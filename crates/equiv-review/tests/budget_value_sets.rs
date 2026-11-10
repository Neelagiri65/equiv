//! Budget gap (sqlfluff replay, 2026-11-10): the Python parser ran past the
//! step budget on one real fixture file, and the whole value-set review ended
//! as "could not check". Inputs drawn from the project's own list are treated
//! like harvested inputs: running out of budget there is "not judged at this
//! input" (counted in the receipt and printed), and the review goes on. A
//! larger budget judges them. Written before the change (holdout).
use equiv_review::{review, ArgType, Domain, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_bvs{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, f: &str, body: &str) -> PathBuf {
    let p = d.join(f);
    std::fs::write(&p, body).unwrap();
    p
}

fn spec(sig: &str, budget: u64) -> ReviewSpec {
    let (t, dm) = ArgType::parse_domain(sig).unwrap();
    ReviewSpec { func: "f".into(), args: vec![t], n: 50, seed: 1, budget, errors_agree: false, domains: vec![dm], same_host: false }
}

// The 4-character name never finishes; every other name is cheap. No literal
// names the slow value, so harvest cannot be what makes it soft.
const SLOW: &str = "def f(s):\n    n = 0\n    if len(s) == len('wxyz'):\n        while True:\n            n += 1\n    return len(s)\n";
const NAMES: &str = "NAMES = ['slow', 'a', 'bb', 'ccc']\n";

#[test]
fn a_value_that_exhausts_the_budget_is_not_judged_and_the_review_goes_on() {
    let d = dir("soft");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", SLOW);
    let r = write(&d, "r.py", SLOW);
    let rec = review(&c, &r, &spec("str[in names.NAMES]", DEFAULT_BUDGET)).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => {
            assert_eq!(not_judged, 1, "the slow name is counted, not hidden");
            assert!(n >= 3, "the other names are still judged: {n}");
        }
        v => panic!("expected EQUIVALENT with one input not judged, got {v:?}"),
    }
}

#[test]
fn a_real_difference_after_a_slow_value_is_still_found() {
    let d = dir("diff");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", &SLOW.replace("return len(s)", "return len(s) + (1 if len(s) == len('ab') else 0)"));
    let r = write(&d, "r.py", SLOW);
    let rec = review(&c, &r, &spec("str[in names.NAMES]", DEFAULT_BUDGET)).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(input.contains("bb"), "{input}"),
        v => panic!("got {v:?}"),
    }
}

#[test]
fn a_larger_budget_judges_a_heavy_value() {
    // Finite but heavy: about 3M line/jump events, under a larger budget only.
    let d = dir("heavy");
    write(&d, "names.py", "NAMES = ['heavy', 'a']\n");
    let body = "def f(s):\n    t = 0\n    if len(s) == len('vwxyz'):\n        for i in range(1500000):\n            t += 1\n    return t\n";
    let c = write(&d, "c.py", body);
    let r = write(&d, "r.py", body);
    let small = review(&c, &r, &spec("str[in names.NAMES]", 1_000_000)).expect("no safety stop");
    assert!(matches!(small.verdict, ReviewVerdict::Equivalent { not_judged: 1, .. }), "{:?}", small.verdict);
    let big = review(&c, &r, &spec("str[in names.NAMES]", 50_000_000)).expect("no safety stop");
    assert!(matches!(big.verdict, ReviewVerdict::Equivalent { not_judged: 0, .. }), "{:?}", big.verdict);
}

#[test]
fn a_review_where_no_input_was_judged_is_never_green() {
    // Every name runs past the budget: nothing was compared, so this is
    // could-not-check, not "agreed on all 0 inputs".
    let d = dir("none");
    write(&d, "names.py", "NAMES = ['wxyz', 'abcd']\n");
    let c = write(&d, "c.py", SLOW);
    let r = write(&d, "r.py", SLOW);
    let rec = review(&c, &r, &spec("str[in names.NAMES]", DEFAULT_BUDGET)).expect("no safety stop");
    match &rec.verdict {
        ReviewVerdict::Error { reason } => assert!(reason.contains("budget"), "{reason}"),
        v => panic!("expected could-not-check, got {v:?}"),
    }
}

#[test]
fn one_side_running_past_the_budget_is_not_judged_and_said_so() {
    // Decided behaviour: a compiled twin raises no Python events, so in twin
    // reviews one side exhausting is the normal case (sqlfluff: the Python
    // parser on a long file). The input is not judged, counted in the receipt
    // and the verdict line, never silently dropped.
    let d = dir("onesided");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", SLOW);
    let r = write(&d, "r.py", "def f(s):\n    return len(s)\n");
    let rec = review(&c, &r, &spec("str[in names.NAMES]", DEFAULT_BUDGET)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { not_judged: 1, .. }), "{:?}", rec.verdict);
    let note = equiv_review::not_judged_note(1);
    assert!(note.contains("not judged"), "{note}");
}
