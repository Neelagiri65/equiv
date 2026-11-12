//! Joint literal cases (0.29.0). A special case guarded by several literals
//! at once (`year == 2024 and month == 2 and day == 29`) used to agree on
//! every generated input: each argument met its literal only while the others
//! sat at a default or a small fixed partner. Runs of consecutive source
//! literals are now tried together. Every diverging case here agreed on all
//! inputs before the change.
//! (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET, DEFAULT_N};

fn run(name: &str, cand: &str, refr: &str, args: Vec<ArgType>) -> ReviewVerdict {
    let d = std::env::temp_dir().join(format!("equiv_joint_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    let (c, r) = (d.join(format!("{name}_c.py")), d.join(format!("{name}_r.py")));
    std::fs::write(&c, cand).unwrap();
    std::fs::write(&r, refr).unwrap();
    let spec = ReviewSpec { func: "f".into(), args, n: DEFAULT_N, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false, hinted: false };
    review(&c, &r, &spec).expect("no safety stop").verdict
}

fn input_of(v: ReviewVerdict) -> String {
    match v {
        ReviewVerdict::Counterexample { input, .. } => input,
        v => panic!("expected a difference, got {v:?}"),
    }
}

const I3: fn() -> Vec<ArgType> = || vec![ArgType::Int, ArgType::Int, ArgType::Int];

#[test]
fn three_int_literals_of_one_condition_meet() {
    let v = run(
        "date",
        "def f(y, m, d):\n    if y == 1999 and m == 12 and d == 31:\n        return -1\n    return y + m + d\n",
        "def f(y, m, d):\n    return y + m + d\n",
        I3(),
    );
    assert_eq!(input_of(v), "(1999, 12, 31)");
}

#[test]
fn the_condition_may_name_the_arguments_in_another_order() {
    let v = run(
        "order",
        "def f(a, b, c):\n    if c == 77 and a == 5005 and b == 313:\n        return 0\n    return a * b - c\n",
        "def f(a, b, c):\n    return a * b - c\n",
        I3(),
    );
    assert_eq!(input_of(v), "(5005, 313, 77)");
}

#[test]
fn negative_literals_meet_with_their_signs() {
    let v = run(
        "neg",
        "def f(a, b):\n    if a == -4040 and b == -909:\n        return 1\n    return 0\n",
        "def f(a, b):\n    return 0\n",
        vec![ArgType::Int, ArgType::Int],
    );
    assert_eq!(input_of(v), "(-4040, -909)");
}

#[test]
fn a_string_and_two_ints_meet() {
    let v = run(
        "mixed",
        "def f(s, a, b):\n    if s == 'DELETE' and a == 9443 and b == 5:\n        return 'x'\n    return 'y'\n",
        "def f(s, a, b):\n    return 'y'\n",
        vec![ArgType::Str, ArgType::Int, ArgType::Int],
    );
    assert_eq!(input_of(v), "('DELETE', 9443, 5)");
}

#[test]
fn two_float_literals_meet() {
    let v = run(
        "floats",
        "def f(lo, hi):\n    if lo == 0.125 and hi == 0.875:\n        return hi\n    return (lo + hi) / 2\n",
        "def f(lo, hi):\n    return (lo + hi) / 2\n",
        vec![ArgType::Float, ArgType::Float],
    );
    assert_eq!(input_of(v), "(0.125, 0.875)");
}

#[test]
fn a_correct_rewrite_of_a_compound_condition_still_agrees_and_stays_close_in_size() {
    let v = run(
        "same",
        "def f(y, m, d):\n    if d == 31 and not m != 12 and 1999 == y:\n        return 1\n    return 0\n",
        "def f(y, m, d):\n    if y == 1999 and m == 12 and d == 31:\n        return 1\n    return 0\n",
        I3(),
    );
    match v {
        // 240 joint cases at most on top of what 0.28.0 generated.
        ReviewVerdict::Equivalent { n, .. } => assert!(n < 2900, "{n} inputs"),
        v => panic!("{v:?}"),
    }
}

#[test]
fn one_argument_functions_get_no_joint_cases() {
    let src = "def f(n):\n    if n == 12345:\n        return 1\n    return 0\n";
    let a = run("one_a", src, src, vec![ArgType::Int]);
    assert!(matches!(a, ReviewVerdict::Equivalent { n, .. } if n < 800), "{a:?}");
}
