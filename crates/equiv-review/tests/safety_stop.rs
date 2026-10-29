//! Constraint tests for the wall-clock safety stop (backlog 0.3b).
//!
//! Work inside a single C call (`sum(range(10**12))`) raises no Python line or
//! jump events, so the deterministic step budget cannot stop it. The Rust side
//! therefore kills the Python process group once one function's check runs
//! past a safety limit. Because that limit is wall-clock time, it must never
//! decide a verdict:
//!   - a safety stop is NOT a receipt: there is nothing to hash, sign or attest;
//!   - the limit is not in the spec hash or the receipt (same bytes for any limit);
//!   - the deterministic step budget still applies first and is unchanged;
//!   - other functions in the same PR run still report.
use equiv_review::{
    render_markdown, review, review_limited, run_pr_limited, ArgType, PrCheck, ReviewSpec,
    ReviewVerdict, DEFAULT_BUDGET,
};
use std::io::Write;
use std::time::{Duration, Instant};

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_safety_{name}.py"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str) -> ReviewSpec {
    ReviewSpec { func: func.into(), args: vec![ArgType::Int], n: 20, seed: 1, budget: DEFAULT_BUDGET }
}

const TERMINATES: &str = "def f(x):\n    return x\n";
const C_LEVEL_LONG: &str = "def f(x):\n    return sum(range(10**12))\n";

// Generous: the limit is 2 s; CI hosts (arm, macOS) can be slow to reap.
const RETURN_WITHIN: Duration = Duration::from_secs(20);

#[test]
fn long_c_level_run_is_stopped_within_the_limit() {
    let cand = tmp("c_long_c", C_LEVEL_LONG);
    let refr = tmp("c_long_r", TERMINATES);
    let t = Instant::now();
    let out = review_limited(&cand, &refr, &spec("f"), 2);
    assert!(t.elapsed() < RETURN_WITHIN, "safety stop took {:?}", t.elapsed());
    let stop = out.expect_err("a run past the safety limit must not produce a receipt");
    let why = stop.reason();
    assert!(why.contains("could not check"), "{why}");
    assert!(why.contains("2 s safety limit"), "reason must state the limit: {why}");
    assert!(why.contains("not part of the deterministic budget"), "{why}");
    assert_eq!(stop.exit_code(), 2, "a safety stop is never green");
}

#[test]
fn whole_process_group_is_killed() {
    // A grandchild that keeps the output pipes open must die too, or the
    // review would hang waiting for end-of-output after Python is killed.
    let cand = tmp("grandchild_c", "import os\ndef f(x):\n    os.system('sleep 300')\n    return x\n");
    let refr = tmp("grandchild_r", TERMINATES);
    let t = Instant::now();
    let out = review_limited(&cand, &refr, &spec("f"), 2);
    assert!(t.elapsed() < RETURN_WITHIN, "group kill took {:?}", t.elapsed());
    assert!(out.is_err(), "expected a safety stop");
}

#[test]
fn safety_limit_is_not_part_of_the_receipt() {
    let cand = tmp("nolimit_c", "def f(x):\n    return x + 0\n");
    let refr = tmp("nolimit_r", TERMINATES);
    let a = review_limited(&cand, &refr, &spec("f"), 1).expect("fast check");
    let b = review_limited(&cand, &refr, &spec("f"), 1000).expect("fast check");
    let c = review(&cand, &refr, &spec("f")).expect("fast check");
    assert!(matches!(a.verdict, ReviewVerdict::Equivalent { .. }));
    assert_eq!(a.to_bytes(), b.to_bytes(), "receipt bytes must not depend on the safety limit");
    assert_eq!(a.to_bytes(), c.to_bytes(), "default limit must give the same receipt");
}

#[test]
fn deterministic_budget_still_decides_python_level_loops() {
    // A Python-level infinite loop is the budget's job: it must still end as a
    // deterministic could-not-check WITH a receipt, long before any safety stop.
    let cand = tmp("pyloop_c", "def f(x):\n    while True:\n        pass\n");
    let refr = tmp("pyloop_r", TERMINATES);
    let r = review_limited(&cand, &refr, &spec("f"), 60).expect("the budget, not the safety stop");
    match &r.verdict {
        ReviewVerdict::Error { reason } => assert!(reason.contains("step budget"), "{reason}"),
        other => panic!("expected step-budget could-not-check, got {other:?}"),
    }
}

#[test]
fn other_functions_in_the_run_still_report() {
    let slow_head = tmp("pr_slow_h", C_LEVEL_LONG);
    let slow_base = tmp("pr_slow_b", TERMINATES);
    let fast_head = tmp("pr_fast_h", "def f(x):\n    return x * 1\n");
    let fast_base = tmp("pr_fast_b", TERMINATES);
    // Slow FIRST: proves the run continues after a safety stop.
    let checks = vec![
        PrCheck { name: "slow".into(), head_path: slow_head, base_path: slow_base, spec: spec("f") },
        PrCheck { name: "fast".into(), head_path: fast_head, base_path: fast_base, spec: spec("f") },
    ];
    let (items, code) = run_pr_limited(&checks, 2);
    assert_eq!(items.len(), 2);
    assert!(items[0].outcome.is_err(), "slow function must be a safety stop");
    let fast = items[1].outcome.as_ref().expect("fast function must still get its receipt");
    assert!(matches!(fast.verdict, ReviewVerdict::Equivalent { .. }));
    assert_eq!(code, 2, "a safety stop blocks the gate as could-not-check");

    let md = render_markdown(&items, Some("deadbeef"));
    assert!(md.contains("2 s safety limit"), "{md}");
    assert!(md.contains("not part of the deterministic budget"), "{md}");
    assert!(md.contains("1 of 2 functions could not be checked"), "{md}");
    let receipts: Vec<&str> = md.lines().filter(|l| l.contains("receipt-id")).collect();
    assert_eq!(receipts.len(), 1, "only the fast function has a receipt: {receipts:?}");
    assert!(receipts[0].contains("`fast`"));
}
