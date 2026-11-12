//! Element values (0.28.0). List elements and dict values used to stay in
//! -20..20 and were never aimed at a source literal, so a filter rewritten
//! from `x >= 100` to `x > 100` agreed on every generated list. Elements are
//! now also drawn from the source's literals and their neighbours and from
//! wider ranges, inside the declared domain. Every diverging case here
//! agreed on all inputs before the change.
//! (require python3 on PATH)
use equiv_review::{review, ArgType, Domain, ReviewReceipt, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET, DEFAULT_N};
use std::path::PathBuf;

fn run_n(name: &str, cand: &str, refr: &str, sig: &str, n: u32) -> ReviewReceipt {
    let d = std::env::temp_dir().join(format!("equiv_elem_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    let (c, r): (PathBuf, PathBuf) = (d.join(format!("{name}_c.py")), d.join(format!("{name}_r.py")));
    std::fs::write(&c, cand).unwrap();
    std::fs::write(&r, refr).unwrap();
    let (t, dm) = ArgType::parse_domain(sig).unwrap();
    let domains = if dm == Domain::Any { vec![] } else { vec![dm] };
    let spec = ReviewSpec { func: "f".into(), args: vec![t], n, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains, same_host: false, hinted: false };
    review(&c, &r, &spec).expect("no safety stop")
}

fn run(name: &str, cand: &str, refr: &str, sig: &str) -> ReviewReceipt {
    run_n(name, cand, refr, sig, DEFAULT_N)
}

fn counterexample(rec: &ReviewReceipt) -> String {
    match &rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => input.clone(),
        v => panic!("expected a counterexample, got {v:?}"),
    }
}

fn judged(rec: &ReviewReceipt) -> u32 {
    match rec.verdict {
        ReviewVerdict::Equivalent { n, .. } => n,
        ref v => panic!("expected equivalent, got {v:?}"),
    }
}

#[test]
fn a_list_element_on_a_source_threshold_is_tried() {
    let refr = "def f(xs):\n    return [x for x in xs if x >= 100]\n";
    let cand = "def f(xs):\n    return [x for x in xs if x > 100]\n";
    let rec = run("thr", cand, refr, "list[int]");
    assert!(counterexample(&rec).contains("100"), "{:?}", rec.verdict);
    assert!(rec.inputs.literal > 0, "elements were not aimed at the literal: {:?}", rec.inputs);
    // A negative threshold too.
    let refr = "def f(xs):\n    return [x for x in xs if x >= -300]\n";
    let cand = "def f(xs):\n    return [x for x in xs if x > -300]\n";
    assert!(counterexample(&run("thrneg", cand, refr, "list[int]")).contains("-300"));
}

#[test]
fn list_elements_leave_the_small_range_without_any_literal() {
    // No number in the source: only a wider element range finds this.
    let refr = "def f(xs):\n    return [len(str(abs(x))) for x in xs]\n";
    let cand = "def f(xs):\n    return [min(len(str(abs(x))), len('ab')) for x in xs]\n";
    counterexample(&run("wide", cand, refr, "list[int]"));
}

#[test]
fn a_dict_value_on_a_source_threshold_is_tried() {
    let refr = "def f(d):\n    return {k: v for k, v in d.items() if v >= 50}\n";
    let cand = "def f(d):\n    return {k: v for k, v in d.items() if v > 50}\n";
    assert!(counterexample(&run("dthr", cand, refr, "dict")).contains("50"));
    // The key and the value must meet: a literal key with a literal value.
    let refr = "def f(d):\n    return d.get('port', 0) >= 8080\n";
    let cand = "def f(d):\n    return d.get('port', 0) > 8080\n";
    assert_eq!(counterexample(&run("dkey", cand, refr, "dict")), "({'port': 8080})");
}

#[test]
fn a_string_literal_is_tried_inside_a_longer_list() {
    // The singleton list holds the literal at index 0, where the two agree.
    let refr = "def f(xs):\n    return xs.index('stop') if 'stop' in xs else -1\n";
    let cand = "def f(xs):\n    return 0 if 'stop' in xs else -1\n";
    assert!(counterexample(&run("lstr", cand, refr, "list[str]")).contains("'stop'"));
}

#[test]
fn element_values_respect_the_declared_domain() {
    let refr = "def f(xs):\n    return [x for x in xs if x >= 100]\n";
    let cand = "def f(xs):\n    return [x for x in xs if x > 100]\n";
    // Inside the domain the threshold is reached...
    assert!(counterexample(&run("dom_in", cand, refr, "list[int[0..150]]")).contains("100"));
    // ...and outside it no element may ever be 100, so the two agree.
    let rec = run("dom_out", cand, refr, "list[int[0..50]]");
    assert!(judged(&rec) > 100, "{:?}", rec.verdict);
    // One-character strings: a longer literal is never put in a list.
    let refr = "def f(xs):\n    return 'stop' in xs\n";
    let rec = run("dom_one", "def f(xs):\n    return False\n", refr, "list[str[1]]");
    assert!(judged(&rec) > 100, "{:?}", rec.verdict);
}

#[test]
fn large_corners_are_tried_as_elements_and_values() {
    // Exact for every element up to 2^53; differs at 2^53 + 1.
    let refr = "def f(xs):\n    return [x * 2 for x in xs]\n";
    let cand = "def f(xs):\n    return [int(x * 2.0) for x in xs]\n";
    assert!(counterexample(&run("big", cand, refr, "list[int]")).contains("9007199254740993"));
    let refr = "def f(d):\n    return {k: v * 2 for k, v in d.items()}\n";
    let cand = "def f(d):\n    return {k: int(v * 2.0) for k, v in d.items()}\n";
    assert!(counterexample(&run("bigd", cand, refr, "dict")).contains("9007199254740993"));
}

#[test]
fn a_correct_rewrite_still_agrees_and_the_case_count_stays_close() {
    let refr = "def f(xs):\n    return [x for x in xs if x >= 250]\n";
    let cand = "def f(xs):\n    out = []\n    for x in xs:\n        if not x < 250:\n            out.append(x)\n    return out\n";
    let n = judged(&run("same", cand, refr, "list[int]"));
    // 1,45x before the change for a source with no literal; the extra cases
    // are bounded (a share of the random count plus the literal-aimed lists).
    assert!((1453..=2180).contains(&n), "list[int] review judged {n} inputs");
    let refr = "def f(d):\n    return sorted(d.items())\n";
    let n = judged(&run("samed", refr, refr, "dict"));
    assert!(n <= 2040, "dict review judged {n} inputs");
}

#[test]
fn an_expensive_element_does_not_end_the_review() {
    // Work grows with the element: a wide element may run out of the step
    // budget. That input is recorded as not judged; the review still ends
    // in a verdict instead of could-not-check.
    let src = "def f(xs):\n    t = 0\n    for x in xs:\n        for _ in range(abs(x)):\n            t += 1\n            t += 1\n            t += 1\n    return t\n";
    let rec = run("cost", src, src, "list[int]");
    match rec.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => assert!(n > 1000 && not_judged > 0, "n {n}, not judged {not_judged}"),
        v => panic!("{v:?}"),
    }
}
