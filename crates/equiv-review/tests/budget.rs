//! Constraint tests for the deterministic step budget (backlog 0.3, gate P3).
//!
//! A function that never returns must not hang the review. It must end as
//! "could not check" with a reason, and the limit must be counted in
//! deterministic units (Python line and jump events), never wall-clock time,
//! so the same budget gives the same verdict on every host. The budget is part
//! of the spec and is written into the receipt. Requires python3 >= 3.12.
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_budget_{name}.py"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>, budget: u64) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 50, seed: 1, budget }
}

fn reason(v: &ReviewVerdict) -> &str {
    match v {
        ReviewVerdict::Error { reason } => reason,
        other => panic!("expected could-not-check, got {other:?}"),
    }
}

const TERMINATES: &str = "def f(x):\n    return x\n";

#[test]
fn infinite_loop_is_could_not_check_with_reason() {
    let cand = tmp("loop_c", "def f(x):\n    while True:\n        pass\n");
    let refr = tmp("loop_r", TERMINATES);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    let why = reason(&r.verdict);
    assert!(why.contains("step budget"), "reason must name the budget: {why}");
    assert!(why.contains(&DEFAULT_BUDGET.to_string()), "reason must state the budget value: {why}");
    assert!(why.contains("candidate"), "reason must say which side ran out: {why}");
    assert_eq!(r.verdict.exit_code(), 2, "could not check is never green");
}

#[test]
fn reference_side_running_out_is_named() {
    let cand = tmp("refloop_c", TERMINATES);
    let refr = tmp("refloop_r", "def f(x):\n    while True: pass\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    let why = reason(&r.verdict);
    assert!(why.contains("reference"), "{why}");
}

#[test]
fn swallowing_the_budget_exception_does_not_escape() {
    // User code that catches everything (even BaseException) and loops again
    // must still be stopped: the budget keeps firing until the call unwinds.
    let cand = tmp(
        "swallow_c",
        "def f(x):\n    while True:\n        try:\n            while True:\n                x += 1\n        except:\n            pass\n",
    );
    let refr = tmp("swallow_r", TERMINATES);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    assert!(reason(&r.verdict).contains("step budget"));
}

#[test]
fn single_line_loops_are_counted() {
    // Loops on one line raise no new LINE events; jumps are counted too.
    for (i, body) in [
        "import itertools\ndef f(x):\n    for _ in itertools.count(): pass\n",
        "import itertools\ndef f(x):\n    return sum(y for y in itertools.count())\n",
        "def f(x):\n    while x or True: pass\n",
    ]
    .iter()
    .enumerate()
    {
        let cand = tmp(&format!("oneline_{i}"), body);
        let refr = tmp(&format!("oneline_r_{i}"), TERMINATES);
        let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
        assert!(reason(&r.verdict).contains("step budget"), "shape {i}");
    }
}

#[test]
fn ordinary_loops_fit_the_default_budget() {
    let cand = tmp("short_c", "def f(n):\n    s = 0\n    for i in range(abs(n) % 1000):\n        s += i\n    return s\n");
    let refr = tmp("short_r", "def f(n):\n    k = abs(n) % 1000\n    return k * (k - 1) // 2\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

#[test]
fn the_budget_decides_the_outcome_and_is_recorded() {
    // Same sources, two budgets: the budget alone moves the verdict, so it is
    // part of what the receipt binds. Same budget twice: byte-identical.
    let src = "def f(n):\n    s = 0\n    for i in range(200):\n        s += i\n    return s + n\n";
    let cand = tmp("bud_c", src);
    let refr = tmp("bud_r", src);
    let tight = review(&cand, &refr, &spec("f", vec![ArgType::Int], 10));
    let roomy = review(&cand, &refr, &spec("f", vec![ArgType::Int], 100_000));
    assert!(reason(&tight.verdict).contains("step budget of 10 "), "{:?}", tight.verdict);
    assert!(matches!(roomy.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", roomy.verdict);
    assert_eq!(tight.budget, 10);
    assert_eq!(roomy.budget, 100_000);
    assert_ne!(tight.spec_sha256, roomy.spec_sha256, "budget is part of the spec");
    let again = review(&cand, &refr, &spec("f", vec![ArgType::Int], 10));
    assert_eq!(tight.to_bytes(), again.to_bytes(), "same budget, same receipt");
}

#[test]
fn a_counterexample_found_first_still_stands() {
    // Diverges on every input except 777, where it would loop. The first case
    // already diverges, so the concrete counterexample is the verdict.
    let cand = tmp("cex_c", "def f(x):\n    if x == 777:\n        while True: pass\n    return x + 1\n");
    let refr = tmp("cex_r", TERMINATES);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
}

#[test]
fn module_that_loops_at_import_is_could_not_check() {
    let cand = tmp("imp_c", "while True:\n    pass\ndef f(x):\n    return x\n");
    let refr = tmp("imp_r", TERMINATES);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    let why = reason(&r.verdict);
    assert!(why.contains("step budget") && why.contains("loading"), "{why}");
}

// The receipt of a budget-exhausted review is pinned. The reason text carries
// the budget, case index, input and side, never the number of events actually
// consumed (that can differ by one or two between Python versions), so this id
// is the same on every host in the CI matrix.
#[test]
fn golden_budget_receipt_id_is_stable() {
    let cand = tmp("gold_c", "def f(x):\n    while True:\n        pass\n");
    let refr = tmp("gold_r", TERMINATES);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int], DEFAULT_BUDGET));
    let id: String = r.sha256().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(id, "01d7f09f7eaa625064936232c907e092816a3886f61b14d384fba963cb4b084b", "receipt-id drifted");
}
