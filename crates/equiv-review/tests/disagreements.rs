//! Every disagreement, not just the first (0.13.0). The first counterexample
//! stays the verdict's headline; the receipt also records how many judged
//! inputs were run and on how many the two sides disagreed, and the full list
//! is available in case order. Requires python3 on PATH.
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_dis{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1, budget: DEFAULT_BUDGET }
}

#[test]
fn counts_every_disagreeing_input_and_keeps_the_first_as_headline() {
    // Differs on exactly three strings, all fixed corners ("", "a", "A").
    let cand = tmp("three_c", "def f(s):\n    return 1 if s in ('', 'a', 'A') else 0\n");
    let refr = tmp("three_r", "def f(s):\n    return 0\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, judged, disagreeing, stopped, .. } => {
            assert_eq!(input, "('')", "the first disagreement stays the headline");
            assert_eq!(*disagreeing, 3);
            assert!(*judged > 100, "every case is judged, not just up to the first: {judged}");
            assert_eq!(stopped, "", "nothing stopped the run");
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
    let listed: Vec<&str> = r.disagreements.iter().map(|d| d.input.as_str()).collect();
    assert_eq!(listed, ["('')", "('a')", "('A')"], "all of them, in case order");
    assert_eq!(r.verdict.exit_code(), 1);
}

#[test]
fn the_count_is_in_the_receipt_bytes() {
    let refr = tmp("bytes_r", "def f(s):\n    return 0\n");
    let one = tmp("bytes_1", "def f(s):\n    return 1 if s == '' else 0\n");
    let two = tmp("bytes_2", "def f(s):\n    return 1 if s in ('', 'a') else 0\n");
    let r1 = review(&one, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    let r2 = review(&two, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    // Same headline input, different counts: the receipts must differ in the
    // verdict, not only in the candidate hash.
    let v = |r: &equiv_review::ReviewReceipt| match &r.verdict {
        ReviewVerdict::Counterexample { input, disagreeing, .. } => (input.clone(), *disagreeing),
        v => panic!("{v:?}"),
    };
    assert_eq!(v(&r1), ("('')".to_string(), 1));
    assert_eq!(v(&r2), ("('')".to_string(), 2));
    let mut a = r1.clone();
    a.candidate_sha256 = r2.candidate_sha256;
    assert_ne!(a.to_bytes(), r2.to_bytes(), "disagreement count must be in the receipt bytes");
}

#[test]
fn a_budget_stop_after_a_disagreement_keeps_the_counterexample_and_says_where_it_stopped() {
    // Diverges everywhere; loops at 777 (a source literal, so a generated input).
    let cand = tmp("stop_c", "def f(x):\n    if x == 777:\n        while True: pass\n    return x + 1\n");
    let refr = tmp("stop_r", "def f(x):\n    return x\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { judged, disagreeing, stopped, .. } => {
            assert_eq!(judged, disagreeing, "every judged input disagrees");
            assert!(*judged >= 1);
            assert!(stopped.contains("step budget") && stopped.contains("777"), "{stopped}");
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
    assert_eq!(r.disagreements.len() as u32, match &r.verdict {
        ReviewVerdict::Counterexample { disagreeing, .. } => *disagreeing,
        _ => unreachable!(),
    });
}

#[test]
fn equivalent_has_no_disagreements() {
    let cand = tmp("eq_c", "def f(s):\n    return s[::-1][::-1]\n");
    let refr = tmp("eq_r", "def f(s):\n    return s\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", r.verdict);
    assert!(r.disagreements.is_empty());
}
