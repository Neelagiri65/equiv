//! Source literals the scan used to lose (0.28.0). A number was read without
//! its sign, so a branch on `-70000` was never tried; a float argument was
//! never offered an int literal or a literal in exponent form; and string
//! escapes were decoded differently from Python (`"\033"` became NUL + "33"),
//! so the text the function tests for was never an input. Every case here
//! agreed on all inputs before the change. A counterexample in these tests is
//! found by the real interpreter comparing the offered input with the
//! literal, so it also shows the decoding matches Python's.
//! (require python3 on PATH)
use equiv_review::{review, ArgType, Domain, ReviewReceipt, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::PathBuf;

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_litreach_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn run(name: &str, cand: &str, refr: &str, sig: &str) -> ReviewReceipt {
    let d = dir();
    let (c, r) = (d.join(format!("{name}_c.py")), d.join(format!("{name}_r.py")));
    std::fs::write(&c, cand).unwrap();
    std::fs::write(&r, refr).unwrap();
    let (t, dm) = ArgType::parse_domain(sig).unwrap();
    let domains = if dm == Domain::Any { vec![] } else { vec![dm] };
    // No random inputs: only corners and literals can find the difference.
    let spec = ReviewSpec { func: "f".into(), args: vec![t], n: 0, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains, same_host: false };
    review(&c, &r, &spec).expect("no safety stop")
}

fn counterexample(rec: &ReviewReceipt) -> String {
    match &rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => input.clone(),
        v => panic!("expected a counterexample, got {v:?}"),
    }
}

const ZERO: &str = "def f(x):\n    return 0\n";

#[test]
fn a_negative_int_literal_is_tried() {
    let refr = "def f(x):\n    return 1 if x == -70000 else 0\n";
    assert_eq!(counterexample(&run("neg", ZERO, refr, "int")), "(-70000)");
    // Also inside a domain that holds only negative ints.
    assert_eq!(counterexample(&run("negdom", ZERO, refr, "int[-100000..-1]")), "(-70000)");
    // And the neighbours of the negative literal.
    let below = "def f(x):\n    return 1 if x < -70000 and x > -70002 else 0\n";
    assert_eq!(counterexample(&run("negnb", ZERO, below, "int")), "(-70001)");
}

#[test]
fn a_domain_still_excludes_the_other_sign() {
    // Both signs are offered, but only inside the declared domain.
    let refr = "def f(x):\n    return 1 if x == -70000 else 0\n";
    let rec = run("posdom", ZERO, refr, "int[1..100000]");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
}

#[test]
fn float_arguments_get_signed_exponent_and_int_literals() {
    for (name, lit, want) in [
        ("fneg", "-0.37", "(-0.37)"),
        ("fexp", "1e-9", "(1e-9)"),
        ("fexpu", "2.5E3", "(2500.0)"),
        ("fint", "100", "(100.0)"),
        ("fnint", "-100", "(-100.0)"),
        ("flead", ".5", "(0.5)"),
        ("ftrail", "5.", "(5.0)"),
        ("funder", "1_000.25", "(1000.25)"),
        ("fbig", "1e300", "(1e300)"),
    ] {
        let refr = format!("def f(x):\n    return 1 if x == {lit} else 0\n");
        assert_eq!(counterexample(&run(name, ZERO, &refr, "float")), want, "literal {lit}");
    }
}

#[test]
fn a_float_boundary_rewrite_is_caught_at_the_literal() {
    for (name, lit) in [("ge_int", "100"), ("ge_exp", "1e-9"), ("ge_neg", "-2.5")] {
        let refr = format!("def f(x):\n    return x >= {lit}\n");
        let cand = format!("def f(x):\n    return x > {lit}\n");
        counterexample(&run(name, &cand, &refr, "float"));
    }
}

#[test]
fn the_floats_next_to_a_literal_are_tried() {
    // The two versions differ only strictly between 0.2499999 and 0.25:
    // neither literal separates them, the float just below 0.25 does.
    let refr = "def f(x):\n    return x >= 0.25\n";
    let cand = "def f(x):\n    return x > 0.2499999\n";
    counterexample(&run("near", cand, refr, "float"));
    // `x > c` and `x >= next float above c` are the same test: no difference
    // may be reported at the literal or beside it.
    let same = "def f(x):\n    return x >= 0.25000000000000006\n";
    let strict = "def f(x):\n    return x > 0.25\n";
    let rec = run("nearsame", same, strict, "float");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
}

#[test]
fn string_escapes_are_decoded_as_python_decodes_them() {
    // Each literal is the only way to the difference; the interpreter decides
    // whether the offered input equals it.
    let lits: &[&str] = &[
        r#""\033""#, r#""\0""#, r#""\7z""#, r#""\101""#, r#""\1011""#, r#""\400""#, r#""\a""#, r#""\b""#, r#""\f""#,
        r#""\v""#, r#""\x41\u00e9""#, r#""\U0001f600!""#, r#""\dz""#, r#""\8""#, r#""q\'q""#, r#"'q\"q'"#,
        r#""q\\q""#, r#"r"\nq""#, r#"R"\tq""#, r#"Rf"\tq""#, r#"fR"\tq""#, r#"rF"\033""#, r#"u"\x7fq""#,
        r#""q\N{BULLET}q" if False else "\x1bq""#,
    ];
    for (k, lit) in lits.iter().enumerate() {
        let refr = format!("def f(s):\n    return s == {lit}\n");
        let rec = run(&format!("esc{k}"), "def f(s):\n    return False\n", &refr, "str");
        assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "literal {lit}: {:?}", rec.verdict);
    }
}

#[test]
fn a_keyword_before_a_quote_is_not_a_prefix() {
    // `or"\tq"`: the letters before the quote are a keyword, so the literal
    // is not raw and holds a tab.
    let refr = "def f(s):\n    return s == (0 or\"\\tq\")\n";
    assert_eq!(counterexample(&run("kw", "def f(s):\n    return False\n", refr, "str")), "('\\x09q')");
}

#[test]
fn bytes_and_undecodable_literals_are_not_offered() {
    // A bytes literal is not a str; a named escape needs a name table; a lone
    // surrogate escape cannot be carried as text. None becomes an input (a
    // wrong guess would be an input the source never names).
    for (k, lit) in [r#"b"zq".decode()"#, r#"rb"zq".decode()"#, r#"Br"zq".decode()"#, r#""\N{BULLET}""#, r#""\ud800""#, r#""\xzz"[:0]"#]
        .iter()
        .enumerate()
    {
        let src = format!("def f(s):\n    return s == {lit}\n");
        let rec = run(&format!("skip{k}"), &src, &src, "str");
        assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. } | ReviewVerdict::Error { .. }), "{lit}: {:?}", rec.verdict);
        assert_eq!(rec.inputs.literal, 0, "{lit} was offered as an input");
    }
}
