//! Cross cases (0.28.0). Corners used to be tried one argument at a time with
//! the others at their defaults (0, "", []), so a corner string never met a
//! non-zero int: `len(s) >= n` against a UTF-16 length agreed on every input.
//! Each argument's corners and literals are now also paired with a few
//! non-degenerate values of every other argument, up to a fixed cap.
//! (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewReceipt, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::PathBuf;

fn run(name: &str, cand: &str, refr: &str, args: Vec<ArgType>, n: u32) -> ReviewReceipt {
    let d = std::env::temp_dir().join(format!("equiv_cross_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    let (c, r): (PathBuf, PathBuf) = (d.join(format!("{name}_c.py")), d.join(format!("{name}_r.py")));
    std::fs::write(&c, cand).unwrap();
    std::fs::write(&r, refr).unwrap();
    let spec = ReviewSpec { func: "f".into(), args, n, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false };
    review(&c, &r, &spec).expect("no safety stop")
}

fn counterexample(rec: &ReviewReceipt) -> String {
    match &rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => input.clone(),
        v => panic!("expected a counterexample, got {v:?}"),
    }
}

const LEN_REF: &str = "def f(s, n):\n    return len(s) >= n\n";

#[test]
fn a_string_corner_meets_a_non_zero_int() {
    let cand = "def f(s, n):\n    return len(s.encode('utf-16-le', 'surrogatepass')) // 2 >= n\n";
    let rec = run("utf16", cand, LEN_REF, vec![ArgType::Str, ArgType::Int], 2000);
    assert_eq!(counterexample(&rec), "('\\U0001f600', 2)");
    // The other way round: an int corner meets a non-empty string.
    let refr = "def f(n, s):\n    return s * n\n";
    let cand = "def f(n, s):\n    return s * n if n != -2 else s\n";
    let rec = run("intstr", cand, refr, vec![ArgType::Int, ArgType::Str], 0);
    assert!(counterexample(&rec).starts_with("(-2, "), "{:?}", rec.verdict);
}

#[test]
fn a_corner_meets_a_literal_of_the_other_argument() {
    // Needs a multi-byte corner string and the int 7, which only the source
    // names.
    let refr = "def f(s, n):\n    return n == 7 and len(s) == 1\n";
    let cand = "def f(s, n):\n    return n == 7 and len(s.encode()) == 1\n";
    let rec = run("lit", cand, refr, vec![ArgType::Str, ArgType::Int], 0);
    assert!(counterexample(&rec).ends_with(", 7)"), "{:?}", rec.verdict);
}

#[test]
fn a_correct_two_argument_rewrite_still_agrees() {
    let cand = "def f(s, n):\n    return not len(s) < n\n";
    let rec = run("same", cand, LEN_REF, vec![ArgType::Str, ArgType::Int], 2000);
    match rec.verdict {
        // 1,990 before the change; the cross cases add a bounded number.
        ReviewVerdict::Equivalent { n, not_judged } => {
            assert!((1990..=2600).contains(&n), "str,int review judged {n} inputs");
            assert_eq!(not_judged, 0);
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn cross_cases_are_capped_whatever_the_signature() {
    let fixed = |args: Vec<ArgType>, params: &str| {
        let src = format!("def f({params}):\n    return 0\n");
        let rec = run(&format!("cap{}", args.len()), &src, &src, args, 0);
        rec.inputs.corner + rec.inputs.literal
    };
    let two = fixed(vec![ArgType::Str, ArgType::Int], "a, b");
    let six = fixed(
        vec![ArgType::Str, ArgType::Int, ArgType::Str, ArgType::ListInt, ArgType::Float, ArgType::ListStr],
        "a, b, c, d, e, g",
    );
    assert!(two < 400, "str,int has {two} fixed inputs");
    // Six arguments have thirty ordered pairs; the cross cases still add at
    // most the cap (480) to the one-at-a-time inputs.
    assert!(six < 1500, "six arguments have {six} fixed inputs");
}

#[test]
fn single_argument_reviews_have_no_cross_cases() {
    let src = "def f(s):\n    return s\n";
    let rec = run("single", src, src, vec![ArgType::Str], 0);
    assert_eq!(rec.inputs.corner, 56, "str corners changed: {:?}", rec.inputs);
}
