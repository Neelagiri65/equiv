//! Honest receipt fields (backlog 1.1). A receipt states what determined its
//! result: the evaluator's Python major.minor (event counts differ between
//! minors), the seed, how many inputs came from each source (corner, literal,
//! random), how many judged inputs raised on a side, and the claim tier. All
//! readable and all in the hash. (require python3 on PATH)
use equiv_review::{render_markdown, review, run_pr, ArgType, InputCounts, PrCheck, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_rf{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false, hinted: false }
}

fn local_python() -> String {
    let out = std::process::Command::new("python3")
        .args(["-c", "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')"])
        .output()
        .expect("python3");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// The version comes from the evaluator that ran the cases, not from a guess.
#[test]
fn receipt_records_the_evaluators_python_major_minor() {
    let c = tmp("py_c", "def f(n):\n    return n + 1\n");
    let r = tmp("py_r", "def f(n):\n    return 1 + n\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    assert_eq!(rc.python, local_python());
    assert_eq!(rc.seed, 1);
    assert_eq!(rc.tier, "tested");
}

// The field is in the hash: the same review on another Python minor is a
// different receipt, by design ("reproducible on the same evaluator version").
#[test]
fn python_field_is_part_of_the_receipt_bytes() {
    let c = tmp("pyh_c", "def f(n):\n    return n\n");
    let r = tmp("pyh_r", "def f(n):\n    return n\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    let mut other = rc.clone();
    other.python = "9.99".into();
    assert_ne!(rc.to_bytes(), other.to_bytes());
    let mut other = rc.clone();
    other.seed = 2;
    assert_ne!(rc.to_bytes(), other.to_bytes());
    let mut other = rc.clone();
    other.exceptions += 1;
    assert_ne!(rc.to_bytes(), other.to_bytes());
    let mut other = rc.clone();
    other.inputs = InputCounts { corner: 0, literal: 0, random: 0, harvested: 0, out_of_domain: 0 };
    assert_ne!(rc.to_bytes(), other.to_bytes());
}

// corner + literal + random is the whole case list: judged plus not judged.
// A source literal (777) shows up in the literal count.
#[test]
fn input_counts_by_source_add_up_to_the_case_list() {
    let c = tmp("cnt_c", "def f(n):\n    return 1 if n == 777 else 0\n");
    let r = tmp("cnt_r", "def f(n):\n    return 1 if n == 777 else 0\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    let (n, not_judged) = match rc.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => (n, not_judged),
        ref v => panic!("{v:?}"),
    };
    let i = &rc.inputs;
    assert_eq!(i.corner + i.literal + i.random, n + not_judged, "{i:?} vs n={n} not_judged={not_judged}");
    assert!(i.literal >= 3, "777 and its neighbours are literal inputs: {i:?}");
    assert!(i.corner >= 5 + 19, "int corners plus the large corners: {i:?}");
    assert!(i.random > 60, "single-int draws dedup heavily, but a full stream is still well over 60: {i:?}");
}

// A function that raises on some inputs and agrees with its reference there
// (same exception type) counts those inputs as exceptions observed.
#[test]
fn exceptions_observed_counts_judged_inputs_where_a_side_raised() {
    let c = tmp("exc_c", "def f(xs):\n    return xs[0]\n");
    let r = tmp("exc_r", "def f(xs):\n    return xs[0] + 0\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::ListInt])).expect("no safety stop");
    assert!(matches!(rc.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rc.verdict);
    assert!(rc.exceptions >= 1, "the empty list raises IndexError on both sides: {}", rc.exceptions);
    let c = tmp("noexc_c", "def g(n):\n    return n\n");
    let r = tmp("noexc_r", "def g(n):\n    return n + 0\n");
    let rc = review(&c, &r, &spec("g", vec![ArgType::Int])).expect("no safety stop");
    assert_eq!(rc.exceptions, 0);
}

// Refused and error receipts carry the evaluator version too (the driver
// reports it before the admissibility check and before the budget check).
#[test]
fn refused_and_error_receipts_carry_the_python_version() {
    let c = tmp("ref_c", "import math\n\ndef f(n):\n    return int(math.log2(abs(n) + 1))\n");
    let r = tmp("ref_r", "def f(n):\n    return 0\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(rc.verdict, ReviewVerdict::Refused { .. }), "{:?}", rc.verdict);
    assert_eq!(rc.python, local_python());
    let c = tmp("err_c", "def f(n):\n    while True:\n        pass\n");
    let r = tmp("err_r", "def f(n):\n    return n\n");
    let rc = review(&c, &r, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(rc.verdict, ReviewVerdict::Error { .. }), "{:?}", rc.verdict);
    assert_eq!(rc.python, local_python());
}

// The PR comment states the provenance next to the verdict.
#[test]
fn pr_markdown_states_python_seed_and_input_sources() {
    let head = tmp("md_h", "def f(n):\n    return n * 2\n");
    let base = tmp("md_b", "def f(n):\n    return n + n\n");
    let checks = vec![PrCheck { name: "f".into(), head_path: head, base_path: base, spec: spec("f", vec![ArgType::Int]), twin: None }];
    let (items, code) = run_pr(&checks);
    assert_eq!(code, 0);
    let md = render_markdown(&items, None);
    let py = local_python();
    assert!(md.contains(&format!("python {py}")), "{md}");
    assert!(md.contains("seed 1"), "{md}");
    assert!(md.contains("corner") && md.contains("literal") && md.contains("random"), "{md}");
}
