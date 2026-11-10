//! Parity-mode noise, step 2 (adoption run 2026-11-10): a compiled twin often
//! rejects inputs its Python reference accepts (day 0, years past int32,
//! multi-character strings where one character is meant). Those out-of-domain
//! counterexamples come first and bury the real differences. Opt-in input
//! domains (`int[lo..hi]`, `str[ascii]`, `str[1]`, also inside lists) keep the
//! generator, corners and harvested literals inside the declared domain.
//! Written before the change (holdout).
use equiv_review::{review, ArgType, Domain, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_dom{}_{name}", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, sig: &[&str]) -> ReviewSpec {
    let (args, domains): (Vec<ArgType>, Vec<Domain>) =
        sig.iter().map(|s| ArgType::parse_domain(s).expect("signature parses")).unzip();
    ReviewSpec {
        func: func.into(),
        args,
        n: 300,
        seed: 1,
        budget: equiv_review::DEFAULT_BUDGET,
        errors_agree: false,
        domains,
        same_host: false,
    }
}

/// All integers that appear in a counterexample's input text.
fn ints_in(s: &str) -> Vec<i128> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '-' && cur.is_empty()) {
            cur.push(ch);
        } else {
            if cur != "-" && !cur.is_empty() {
                out.push(cur.parse().unwrap());
            }
            cur.clear();
        }
    }
    out
}

// The twin (candidate) rejects years outside 1..=9999; the Python reference
// accepts any int. Inside the range they differ on every x with x % 7 == 3:
// a real, in-domain difference.
const TWIN: &str = "def f(x):\n    if x < 1 or x > 9999:\n        raise ValueError('year out of range')\n    return x * 2 + (1 if x % 7 == 3 else 0)\n";
const REFR: &str = "def f(x):\n    return x * 2\n";
const TWIN_OK: &str = "def f(x):\n    if x < 1 or x > 9999:\n        raise ValueError('year out of range')\n    return x * 2\n";

// --- parsing ---------------------------------------------------------------

#[test]
fn parses_domains_and_keeps_plain_types_unconstrained() {
    assert_eq!(ArgType::parse_domain("int"), Some((ArgType::Int, Domain::Any)));
    assert_eq!(ArgType::parse_domain("int[1..9999]"), Some((ArgType::Int, Domain::IntRange(1, 9999))));
    assert_eq!(ArgType::parse_domain("int[-5..5]"), Some((ArgType::Int, Domain::IntRange(-5, 5))));
    assert_eq!(ArgType::parse_domain("str[ascii]"), Some((ArgType::Str, Domain::Ascii)));
    assert_eq!(ArgType::parse_domain("str[1]"), Some((ArgType::Str, Domain::OneChar)));
    assert_eq!(ArgType::parse_domain("list[int[1..12]]"), Some((ArgType::ListInt, Domain::IntRange(1, 12))));
    assert_eq!(ArgType::parse_domain("list[str[1]]"), Some((ArgType::ListStr, Domain::OneChar)));
    assert_eq!(ArgType::parse_domain("list[str[ascii]]"), Some((ArgType::ListStr, Domain::Ascii)));
    // Plain parse is unchanged.
    assert_eq!(ArgType::parse("list[int]"), Some(ArgType::ListInt));
}

#[test]
fn rejects_malformed_or_meaningless_domains() {
    assert_eq!(ArgType::parse_domain("int[9..1]"), None); // empty range
    assert_eq!(ArgType::parse_domain("int[1..]"), None);
    assert_eq!(ArgType::parse_domain("int[ascii]"), None); // wrong kind
    assert_eq!(ArgType::parse_domain("str[1..9]"), None);
    assert_eq!(ArgType::parse_domain("float[0..1]"), None); // not in scope
    assert_eq!(ArgType::parse_domain("bool[1..2]"), None);
}

// --- the holdout case ------------------------------------------------------

#[test]
fn unconstrained_first_counterexample_is_out_of_domain() {
    // Today's behaviour, the reason for this change: the first thing shown is
    // an input the maintainer would dismiss.
    let c = tmp("u_c.py", TWIN);
    let r = tmp("u_r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["int"])).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => {
            let xs = ints_in(&input);
            assert!(xs.iter().any(|&x| !(1..=9999).contains(&x)), "expected out-of-range first, got {input}");
        }
        v => panic!("got {v:?}"),
    }
}

#[test]
fn int_range_leads_with_the_in_domain_difference() {
    let c = tmp("d_c.py", TWIN);
    let r = tmp("d_r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["int[1..9999]"])).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => {
            let xs = ints_in(&input);
            assert!(!xs.is_empty() && xs.iter().all(|&x| (1..=9999).contains(&x)), "expected in-range, got {input}");
            assert!(xs.iter().all(|&x| x % 7 == 3), "expected the real difference, got {input}");
        }
        v => panic!("got {v:?}"),
    }
}

#[test]
fn int_range_twins_that_agree_in_domain_pass() {
    let c = tmp("ok_c.py", TWIN_OK);
    let r = tmp("ok_r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["int[1..9999]"])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
}

#[test]
fn range_endpoints_are_judged() {
    // Corners are clipped to the range, not dropped: lo and hi are inputs.
    // The range is wide (random draws won't saturate it) and the endpoints are
    // built from strings so the int-literal scan cannot supply them.
    let c = tmp("e_c.py", "def f(x):\n    return -1 if str(x) in ('3' + '7', '9000' + '00') else x\n");
    let r = tmp("e_r.py", "def f(x):\n    return x\n");
    let both = review(&c, &r, &spec("f", &["int[37..900000]"])).expect("no safety stop");
    assert!(matches!(both.verdict, ReviewVerdict::Counterexample { disagreeing: 2, .. }), "endpoints not both judged: {:?}", both.verdict);
    let none = review(&c, &r, &spec("f", &["int[38..899999]"])).expect("no safety stop");
    assert!(matches!(none.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", none.verdict);
}

// The pendulum helper shape: (year, month, day), each with its own domain.
const YMD_TWIN: &str = "def f(y, m, d):\n    if not (1 <= y <= 9999 and 1 <= m <= 12 and 1 <= d <= 28):\n        raise ValueError('date out of range')\n    return y * 10000 + m * 100 + d\n";
const YMD_TWIN_DIFF: &str = "def f(y, m, d):\n    if not (1 <= y <= 9999 and 1 <= m <= 12 and 1 <= d <= 28):\n        raise ValueError('date out of range')\n    return y * 10000 + m * 100 + d + (1 if d % 5 == 2 else 0)\n";
const YMD_REFR: &str = "def f(y, m, d):\n    return y * 10000 + m * 100 + d\n";
const YMD_SIG: [&str; 3] = ["int[1..9999]", "int[1..12]", "int[1..28]"];

#[test]
fn multi_arg_unconstrained_leads_out_of_domain() {
    let c = tmp("m0_c.py", YMD_TWIN);
    let r = tmp("m0_r.py", YMD_REFR);
    let rec = review(&c, &r, &spec("f", &["int", "int", "int"])).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => {
            let xs = ints_in(&input);
            assert_eq!(xs.len(), 3, "input format: {input}");
            let inside = (1..=9999).contains(&xs[0]) && (1..=12).contains(&xs[1]) && (1..=28).contains(&xs[2]);
            assert!(!inside, "expected out-of-domain first, got {input}");
        }
        v => panic!("got {v:?}"),
    }
}

#[test]
fn multi_arg_domains_pass_clean_twins() {
    let c = tmp("m1_c.py", YMD_TWIN);
    let r = tmp("m1_r.py", YMD_REFR);
    let rec = review(&c, &r, &spec("f", &YMD_SIG)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
}

#[test]
fn multi_arg_domains_lead_with_the_in_domain_difference() {
    let c = tmp("m2_c.py", YMD_TWIN_DIFF);
    let r = tmp("m2_r.py", YMD_REFR);
    let rec = review(&c, &r, &spec("f", &YMD_SIG)).expect("no safety stop");
    match rec.verdict {
        ReviewVerdict::Counterexample { input, .. } => {
            let xs = ints_in(&input);
            assert_eq!(xs.len(), 3, "input format: {input}");
            assert!((1..=9999).contains(&xs[0]) && (1..=12).contains(&xs[1]) && (1..=28).contains(&xs[2]), "got {input}");
            assert_eq!(xs[2] % 5, 2, "expected the real difference, got {input}");
        }
        v => panic!("got {v:?}"),
    }
}

#[test]
fn harvested_literals_outside_the_domain_are_dropped() {
    // 50000 appears in the source, so harvest would feed it in; it is outside
    // int[1..9999] and must not be judged.
    let c = tmp("h_c.py", "def f(x):\n    if x == 50000:\n        return -1\n    return x\n");
    let r = tmp("h_r.py", "def f(x):\n    return x\n");
    let plain = review(&c, &r, &spec("f", &["int"])).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Counterexample { .. }), "harvest baseline: {:?}", plain.verdict);
    let dom = review(&c, &r, &spec("f", &["int[1..9999]"])).expect("no safety stop");
    assert!(matches!(dom.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", dom.verdict);
}

#[test]
fn list_elements_respect_the_domain() {
    let c = tmp("l_c.py", "def f(xs):\n    for x in xs:\n        if x < 1 or x > 12:\n            raise ValueError('month')\n    return sum(xs)\n");
    let r = tmp("l_r.py", "def f(xs):\n    return sum(xs)\n");
    let plain = review(&c, &r, &spec("f", &["list[int]"])).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", plain.verdict);
    let dom = review(&c, &r, &spec("f", &["list[int[1..12]]"])).expect("no safety stop");
    assert!(matches!(dom.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", dom.verdict);
}

#[test]
fn ascii_strings_stay_ascii() {
    let c = tmp("a_c.py", "def f(s):\n    s.encode('ascii')\n    return s.upper()\n");
    let r = tmp("a_r.py", "def f(s):\n    return s.upper()\n");
    let plain = review(&c, &r, &spec("f", &["str"])).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", plain.verdict);
    let dom = review(&c, &r, &spec("f", &["str[ascii]"])).expect("no safety stop");
    assert!(matches!(dom.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", dom.verdict);
}

#[test]
fn one_char_strings_in_lists() {
    // The charset_normalizer alphabet_languages shape: a list of characters.
    let c = tmp("o_c.py", "def f(cs):\n    return sorted(ord(c) for c in cs)\n");
    let r = tmp("o_r.py", "def f(cs):\n    return sorted(ord(c[0]) for c in cs if c)\n");
    let plain = review(&c, &r, &spec("f", &["list[str]"])).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", plain.verdict);
    let dom = review(&c, &r, &spec("f", &["list[str[1]]"])).expect("no safety stop");
    assert!(matches!(dom.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", dom.verdict);
}

// --- receipt ---------------------------------------------------------------

#[test]
fn domains_change_the_receipt_and_unconstrained_does_not() {
    // A pass on a constrained domain must never be mistaken for a pass on the
    // whole type. `int` and an explicit all-Any domain list hash the same as
    // before (the goldens in budget/floats/objects/tuples guard old ids).
    let c = tmp("rid_c.py", "def f(x):\n    return x * 2\n");
    let r = tmp("rid_r.py", "def f(x):\n    return x + x\n");
    let plain = review(&c, &r, &spec("f", &["int"])).expect("no safety stop");
    let mut empty = spec("f", &["int"]);
    empty.domains = vec![];
    let empty = review(&c, &r, &empty).expect("no safety stop");
    let dom = review(&c, &r, &spec("f", &["int[1..9999]"])).expect("no safety stop");
    let dom2 = review(&c, &r, &spec("f", &["int[1..9998]"])).expect("no safety stop");
    assert_eq!(plain.sha256(), empty.sha256());
    assert_ne!(plain.sha256(), dom.sha256());
    assert_ne!(dom.sha256(), dom2.sha256());
}

#[test]
fn domain_position_is_bound_into_the_receipt() {
    let c = tmp("pos_c.py", "def f(a, b):\n    return a + b\n");
    let r = tmp("pos_r.py", "def f(a, b):\n    return b + a\n");
    let first = review(&c, &r, &spec("f", &["int[1..9]", "int"])).expect("no safety stop");
    let second = review(&c, &r, &spec("f", &["int", "int[1..9]"])).expect("no safety stop");
    assert_ne!(first.sha256(), second.sha256());
}

// --- coverage inside wide ranges and harvested slots (advisor review) -------

#[test]
fn wide_ranges_keep_the_small_magnitude_spread() {
    // `int[0..2147483647]` must still reach ordinary values (3-digit here),
    // as unconstrained `int` does; a draw that only hugs the endpoints or is
    // uniform over 2^31 would pass this twin as equivalent. No int literals,
    // so the literal scan cannot supply the value.
    let c = tmp("w_c.py", "def f(x):\n    return x + (len('x') if len(str(x)) == len('abc') else 0)\n");
    let r = tmp("w_r.py", "def f(x):\n    return x\n");
    let rec = review(&c, &r, &spec("f", &["int[0..2147483647]"])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", rec.verdict);
    let neg = review(&c, &r, &spec("f", &["int[-2147483648..-1]"])).expect("no safety stop");
    // '-12' has three characters: small negatives must be reached as well.
    assert!(matches!(neg.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", neg.verdict);
}

#[test]
fn harvested_slots_outside_the_domain_are_skipped_and_counted() {
    use equiv_review::{review_with_inputs, PyVal, Slots};
    let c = tmp(
        "hs_c.py",
        "def f(a, b):\n    return a + b + (len('xy') if str(a) in ('50000', '5000000') or str(b) == '70000' else 0)\n",
    );
    let r = tmp("hs_r.py", "def f(a, b):\n    return a + b\n");
    let int = |x: i64| Some(PyVal::Int(x));
    let slots: Vec<Slots> = vec![
        vec![int(50000), int(2)],   // complete call, inside the envelope
        vec![None, int(70000)],     // one argument only
        vec![int(5_000_000), int(3)], // complete call, outside the envelope (tail)
    ];
    let plain = spec("f", &["int", "int"]);
    let base = review_with_inputs(&c, &r, &plain, &slots, 60).expect("no safety stop");
    assert!(matches!(base.verdict, ReviewVerdict::Counterexample { .. }), "harvest baseline: {:?}", base.verdict);

    let dom = spec("f", &["int[1..9999]", "int[1..9999]"]);
    let none = review_with_inputs(&c, &r, &dom, &[], 60).expect("no safety stop");
    let rec = review_with_inputs(&c, &r, &dom, &slots, 60).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
    // Two complete calls and three single values fall outside.
    assert_eq!(rec.inputs.out_of_domain - none.inputs.out_of_domain, 5, "{:?} vs {:?}", rec.inputs, none.inputs);
    // In-domain harvested values are still used.
    assert!(rec.inputs.harvested > 0, "{:?}", rec.inputs);
}
