//! Boundary + source-literal input generation: divergences the old seeded
//! random generator could not reach (magic constants, strings longer than 8,
//! uppercase, lists longer than 6) are now caught. These are the missing
//! inputs that used to pass green. (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_t{}_{name}", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false }
}

// A divergence that lives only at a magic constant (x == 777). Uniform random
// over +/-1e6 hits it with ~negligible probability; the literal scan reads 777
// out of the source and tests it directly.
#[test]
fn literal_constant_divergence_is_caught() {
    let cand = tmp("gen_lit_cand.py", "def f(x):\n    return 1 if x == 777 else 0\n");
    let refr = tmp("gen_lit_ref.py", "def f(x):\n    return 0\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(
            input.contains("777"),
            "expected the magic constant as the counterexample, got {input}"
        ),
        v => panic!("literal-derived input should catch this, got {v:?}"),
    }
}

// Diverges only for strings longer than 8. The random generator caps length at
// 8, so it can never produce a counterexample; the boundary case (len 10) does.
#[test]
fn long_string_divergence_is_caught() {
    let cand = tmp("gen_long_cand.py", "def f(s):\n    return len(s)\n");
    let refr = tmp("gen_long_ref.py", "def f(s):\n    return min(len(s), 8)\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "a >8-char boundary string should catch this, got {:?}",
        r.verdict
    );
}

// Diverges on any uppercase input. The random generator only emits a-z, so it
// never sees an uppercase letter; the boundary case "A" does.
#[test]
fn uppercase_divergence_is_caught() {
    let cand = tmp("gen_upper_cand.py", "def f(s):\n    return s.lower()\n");
    let refr = tmp("gen_upper_ref.py", "def f(s):\n    return s\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "an uppercase boundary string should catch this, got {:?}",
        r.verdict
    );
}

// Diverges only when the list is longer than 6. Random lists are <=6 long, so
// they miss it; the boundary case (length 10) catches it.
#[test]
fn long_list_divergence_is_caught() {
    let cand = tmp("gen_llist_cand.py", "def f(xs):\n    return sum(xs)\n");
    let refr = tmp("gen_llist_ref.py", "def f(xs):\n    return sum(xs[:6])\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::ListInt])).expect("no safety stop");
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "a >6-element boundary list should catch this, got {:?}",
        r.verdict
    );
}

// A genuinely behaviour-preserving refactor must still pass: the new corner
// inputs must not manufacture false counterexamples.
#[test]
fn equivalent_refactor_still_passes() {
    let cand = tmp("gen_eq_cand.py", "def f(n):\n    return n * 2\n");
    let refr = tmp("gen_eq_ref.py", "def f(n):\n    return n + n\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    assert!(
        matches!(r.verdict, ReviewVerdict::Equivalent { .. }),
        "equivalent refactor must stay green, got {:?}",
        r.verdict
    );
}

// AC-1: adding deterministic corner + literal cases must keep receipts
// byte-identical run to run.
#[test]
fn determinism_preserved_with_enumeration() {
    let cand = tmp("gen_det_cand.py", "def f(x):\n    return 1 if x == 5 else x\n");
    let refr = tmp("gen_det_ref.py", "def f(x):\n    return x\n");
    let s = spec("f", vec![ArgType::Int]);
    let r1 = review(&cand, &refr, &s).expect("no safety stop");
    let r2 = review(&cand, &refr, &s).expect("no safety stop");
    assert_eq!(r1.to_bytes(), r2.to_bytes(), "receipts must be reproducible");
    assert_eq!(r1.sha256(), r2.sha256());
}

// Magnitude safety: an O(n) reference must not hang on the enumerated inputs.
// Boundary/literal ints stay inside the envelope, so range(abs(n)) is bounded
// there and the function is judged everywhere it was before. The large-
// magnitude corners (up to 2^63 - 1) run last; the reference runs out of
// its deterministic step budget on each, and every such input is counted as
// not judged in the verdict and the receipt. Equivalent, in bounded time,
// with the gap stated: never a hang, never silently green, never an error
// for a function that is fine inside the declared envelope.
#[test]
fn linear_reference_does_not_hang() {
    let cand = tmp("gen_lin_cand.py", "def f(n):\n    return abs(n)\n");
    let refr = tmp(
        "gen_lin_ref.py",
        "def f(n):\n    c = 0\n    for _ in range(abs(n)): c += 1\n    return c\n",
    );
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => {
            assert!(*not_judged > 0, "the reference cannot count to 2^63 inside the budget");
            assert!(*n > 100, "the envelope cases are still judged, got {n}");
        }
        v => panic!("expected equivalent with unjudged corners, got {v:?}"),
    }
}

// The not-judged count is zero for a function that finishes at every corner,
// and the receipt carries the count (it is in the hash, so two runs that
// differ only in how many corners finished have different receipt ids).
#[test]
fn not_judged_is_zero_when_every_corner_finishes_and_is_in_the_receipt() {
    let cand = tmp("gen_nj_cand.py", "def e(n):\n    return not n & 1\n");
    let refr = tmp("gen_nj_ref.py", "def e(n):\n    return n % 2 == 0\n");
    let r = review(&cand, &refr, &spec("e", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { not_judged: 0, .. }), "got {:?}", r.verdict);
    let a = ReviewVerdict::Equivalent { n: 120, not_judged: 0 };
    let b = ReviewVerdict::Equivalent { n: 120, not_judged: 3 };
    let mut ra = r.clone();
    ra.verdict = a;
    let mut rb = r.clone();
    rb.verdict = b;
    assert_ne!(ra.to_bytes(), rb.to_bytes(), "not_judged must be part of the receipt bytes");
}

// The repo's own gated example: a loop summed 1..n against its closed form.
// Equivalent inside the envelope; at the 19 large corners the loop cannot
// finish, and the verdict says so instead of failing the review.
#[test]
fn loop_to_closed_form_is_equivalent_with_the_corners_recorded_as_not_judged() {
    let cand = tmp("gen_tot_cand.py", "def total(n):\n    return n*(n+1)//2 if n>0 else 0\n");
    let refr = tmp("gen_tot_ref.py", "def total(n):\n    s=0\n    for i in range(1,max(n,0)+1): s+=i\n    return s\n");
    let r = review(&cand, &refr, &spec("total", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        // Negative corners finish at once (range is empty); the positive
        // ones above the budget do not: 2^31-1 and up, nine of them.
        ReviewVerdict::Equivalent { not_judged, .. } => assert_eq!(*not_judged, 9, "got {:?}", r.verdict),
        v => panic!("expected equivalent, got {v:?}"),
    }
}

// Budget exhaustion INSIDE the envelope is still an error: the declared scope
// is judged or the review says it could not judge it. Only the corners past
// the envelope get the per-input treatment.
#[test]
fn budget_exhaustion_inside_the_envelope_is_still_an_error() {
    // A five-line loop body: past the stated scope of the default budget
    // (a loop over |n| up to 1e6 with a one-line body fits).
    let body = "def f(n):\n    t = 0\n    for i in range(abs(n)):\n        t += i\n        t -= 1\n        t += 1\n        t ^= 0\n        t |= 0\n    return t\n";
    let cand = tmp("gen_inb_cand.py", body);
    let refr = tmp("gen_inb_ref.py", body);
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Error { reason } => assert!(reason.contains("step budget"), "{reason}"),
        v => panic!("a five-line loop body over a random |n| near 1e6 must exhaust the budget inside the envelope, got {v:?}"),
    }
}

// Numeric text: signed, decimal, exponent and whitespace-padded numbers are
// where hand-written number parsing and padding diverge from int()/float()/
// zfill(). Each must be a generated string input on its own, not only when the
// source happens to spell it. The probe builds the target text from code
// points, so no string literal in the source names it (the literal scan cannot
// supply it); the only way to a counterexample is a fixed string corner.
#[test]
fn numeric_text_corners_are_generated() {
    for text in ["-1", "+1", "1.5", "1e3", " 7 "] {
        let codes: Vec<String> = text.chars().map(|c| (c as u32).to_string()).collect();
        let cand = tmp(
            "gen_num_cand.py",
            &format!("def f(s):\n    return s == \"\".join(map(chr, [{}]))\n", codes.join(", ")),
        );
        let refr = tmp("gen_num_ref.py", "def f(s):\n    return False\n");
        let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
        let want = format!("('{text}')");
        match &r.verdict {
            ReviewVerdict::Counterexample { input, .. } => assert_eq!(
                input, &want,
                "the counterexample should be the numeric-text corner itself"
            ),
            v => panic!("numeric text {text:?} should be a generated input, got {v:?}"),
        }
    }
}

// zfill rewritten as rjust(..., "0") only diverges on a leading sign: zfill
// keeps the sign in front of the zeros, rjust puts the zeros before it.
#[test]
fn leading_sign_padding_divergence_is_caught() {
    let cand = tmp("gen_sign_cand.py", "def pad(s):\n    return s.rjust(5, \"0\")\n");
    let refr = tmp("gen_sign_ref.py", "def pad(s):\n    return s.zfill(5)\n");
    let r = review(&cand, &refr, &spec("pad", vec![ArgType::Str])).expect("no safety stop");
    assert!(
        matches!(r.verdict, ReviewVerdict::Counterexample { .. }),
        "a leading-sign string corner should catch zfill vs rjust, got {:?}",
        r.verdict
    );
}

// Joint numeric corners (backlog 0.5). One-at-a-time enumeration holds the
// other numeric arguments at their default (int 0, float 1.0), so a divergence
// that needs two arguments to be related (cancelling magnitudes, equal values)
// is never reached except by luck in the random tail. For every pair of
// same-type numeric arguments equiv now also runs a small fixed joint set.
// Float reassociation is the canonical case: a + b + c vs a + (b + c) differ
// when a and b cancel and c is small. The seed is irrelevant: these are
// fixed, not random, so the catch is the same on every host.
#[test]
fn float_cancelling_pair_is_generated_for_every_pair() {
    // (a + b) + c vs a + (b + c): differs at (x, -x, 1.0) once x is large
    // enough that x - 1 rounds back to x (2^54 is the smallest power of two
    // where that holds on both sides).
    let cand = tmp("gen_joint_sum_cand.py", "def f(a, b, c):\n    return a + (b + c)\n");
    let refr = tmp("gen_joint_sum_ref.py", "def f(a, b, c):\n    return a + b + c\n");
    let spec3 = spec("f", vec![ArgType::Float, ArgType::Float, ArgType::Float]);
    let r = review(&cand, &refr, &spec3).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert_eq!(
            input, "(1.8014398509481984e16, -1.8014398509481984e16, 1.0)",
            "the first diverging input should be the joint cancelling corner, not a random draw"
        ),
        v => panic!("float reassociation must be caught by the joint corners, got {v:?}"),
    }
    // The related pair can be the first and LAST argument: the joint set
    // must cover every pair, not only neighbours. Only a == c far outside
    // the single-argument sweep (which holds c at 1.0) exposes this one.
    let cand = tmp(
        "gen_joint_far_cand.py",
        "def f(a, b, c):\n    return 1.0 if a == c and abs(a) > 1e6 else 0.0\n",
    );
    let refr = tmp("gen_joint_far_ref.py", "def f(a, b, c):\n    return 0.0\n");
    let r = review(&cand, &refr, &spec3).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert_eq!(
            input, "(1.8014398509481984e16, 1.0, 1.8014398509481984e16)",
            "the (first, last) pair should get the joint corners too"
        ),
        v => panic!("a divergence on the (first, last) pair must be caught, got {v:?}"),
    }
}

#[test]
fn int_equal_pair_is_generated() {
    // Only diverges when both ints are equal and non-zero. The one-at-a-time
    // sweep never has both non-zero; a random ±12 draw hits a == b with
    // probability ~1/25 per case, so it is probably caught by luck, but the
    // joint corner makes the FIRST counterexample the fixed one.
    let cand = tmp("gen_joint_eq_cand.py", "def f(a, b):\n    return a - b if a == b and a != 0 else 0\n");
    let refr = tmp("gen_joint_eq_ref.py", "def f(a, b):\n    return 1 if a == b and a != 0 else 0\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int, ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert_eq!(input, "(1, 1)"),
        v => panic!("equal-int pair must be a joint corner, got {v:?}"),
    }
}

#[test]
fn joint_corners_do_not_touch_single_argument_reviews() {
    // A single numeric argument has no pair: the case list, and so the
    // receipt, must be identical to before. The goldens in floats.rs and
    // objects.rs pin this across the version; here the count is checked.
    let cand = tmp("gen_joint_single_cand.py", "def f(a):\n    return a * 2\n");
    let refr = tmp("gen_joint_single_ref.py", "def f(a):\n    return a + a\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Float])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Equivalent { n, .. } => assert_eq!(*n, 71, "single-float case count changed (66 + 5 tie corners in 0.20)"),
        v => panic!("got {v:?}"),
    }
}

#[test]
fn joint_corners_cause_no_false_alarm_on_exact_rewrites() {
    // Commutative float multiplication and int binomial expansion are exact
    // for every pair, including the joint corners (inf, NaN, MAX, cancelling).
    let cand = tmp("gen_joint_mul_cand.py", "def f(a, b):\n    return b * a\n");
    let refr = tmp("gen_joint_mul_ref.py", "def f(a, b):\n    return a * b\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Float, ArgType::Float])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    let cand = tmp("gen_joint_sq_cand.py", "def f(a, b):\n    return a * a + 2 * a * b + b * b\n");
    let refr = tmp("gen_joint_sq_ref.py", "def f(a, b):\n    return (a + b) * (a + b)\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int, ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// The literal scan must read every literal shape a branch constant is
// commonly written in, not just a bare decimal: radix prefixes, underscores,
// and literal-op-literal folds. Each pair differs only at that constant, far
// outside the boundary corners and (deterministically) outside the random
// stream, so only the scan can reach it.
#[test]
fn literal_scan_reads_radix_underscore_and_folded_constants() {
    for (tag, lit, value) in [
        ("hex", "0xFF", "255"),
        ("underscore", "1_000", "1000"),
        ("shift", "1 << 19", "524288"),
        ("pow", "2 ** 31", "2147483648"),
        ("mul", "60 * 60", "3600"),
        ("octal", "0o777", "511"),
        ("binary", "0b1010_1010", "170"),
    ] {
        let cand = tmp(&format!("gen_rad_{tag}_cand.py"), &format!("def f(x):\n    return 1 if x == {lit} else 0\n"));
        let refr = tmp(&format!("gen_rad_{tag}_ref.py"), "def f(x):\n    return 0\n");
        let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
        match &r.verdict {
            ReviewVerdict::Counterexample { input, .. } => {
                assert_eq!(input, &format!("({value})"), "{tag}: wrong counterexample for literal {lit}")
            }
            v => panic!("{tag}: literal {lit} should be reached by the scan, got {v:?}"),
        }
    }
}

// A string literal in the source becomes an input by value; a dict keyed by a
// source string literal with a falsy value reaches `d.get(k) or default`.
#[test]
fn literal_scan_reads_string_literals_for_str_and_dict_inputs() {
    let cand = tmp("gen_strlit_cand.py", "def f(s):\n    return 1 if s == \"xyzzy\" else 0\n");
    let refr = tmp("gen_strlit_ref.py", "def f(s):\n    return 0\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Str])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(input.contains("xyzzy"), "got {input}"),
        v => panic!("string literal should be reached by the scan, got {v:?}"),
    }
    let cand = tmp("gen_dictlit_cand.py", "def f(d):\n    return d.get(\"port\") or 8080\n");
    let refr = tmp("gen_dictlit_ref.py", "def f(d):\n    return d.get(\"port\", 8080)\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Dict])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, candidate, reference, .. } => {
            assert!(input.contains("port"), "got {input}");
            assert_eq!(candidate, "8080");
            assert_eq!(reference, "0");
        }
        v => panic!("dict literal key with a falsy value should be reached, got {v:?}"),
    }
}

// Under an int signature the scan walks past float literals, exponents,
// identifiers that start with a digit and escaped string literals without
// stalling or mis-tokenising; the review completes and is equivalent.
#[test]
fn int_signature_source_with_float_and_escape_literals_reviews_cleanly() {
    let body = "def f(x):\n    y = 0.5 + 1e3 + 2.\n    x1 = \"a\\n\" + 'b\\\\' + \"\"\n    z = [1.0, 3e-2]\n    return x + 0 if x != 1e3 else x\n";
    let cand = tmp("gen_mixed_cand.py", body);
    let refr = tmp("gen_mixed_ref.py", "def f(x):\n    return x\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// Type-invalid inputs must never reach a function: `a + b` and `b + a` agree
// on every pair of ints but not on strings, so a string pair leaking into
// int slots would show as a false counterexample.
#[test]
fn commutative_int_add_has_no_false_counterexample() {
    let cand = tmp("gen_comm_cand.py", "def f(a, b):\n    return a + b\n");
    let refr = tmp("gen_comm_ref.py", "def f(a, b):\n    return b + a\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int, ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    let cand = tmp("gen_comm3_cand.py", "def f(a, b, c):\n    return a + b + len(c)\n");
    let refr = tmp("gen_comm3_ref.py", "def f(a, b, c):\n    return len(c) + b + a\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int, ArgType::Int, ArgType::Str])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// An even seed runs a full random stream end to end (the suite otherwise
// only ever uses seed 1): the honest tally n stays close to seed 1's.
#[test]
fn even_seed_checks_a_full_set_of_cases() {
    let cand = tmp("gen_seed_cand.py", "def f(a):\n    return a * 2\n");
    let refr = tmp("gen_seed_ref.py", "def f(a):\n    return a + a\n");
    let n_of = |seed: u64| {
        let s = ReviewSpec { func: "f".into(), args: vec![ArgType::Int], n: 200, seed, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false };
        match review(&cand, &refr, &s).expect("no safety stop").verdict {
            ReviewVerdict::Equivalent { n, .. } => n,
            v => panic!("got {v:?}"),
        }
    };
    // Single-int random draws dedup heavily (the small arm has 25 values), so
    // the tally is well below n for every seed; a collapsed stream would be
    // the handful of corner cases only.
    let (n1, n2) = (n_of(1), n_of(2));
    assert!(n1 > 60, "seed 1 checked only {n1} cases");
    assert!(n2 * 10 >= n1 * 8, "seed 2 checked {n2} cases against {n1} for seed 1: the random stream collapsed");
}

// Large-magnitude int corners (backlog 0.5b). The random envelope is
// +/-1,000,000, so a divergence that lives only above 2^53 (where float64
// stops being exact for ints) is unreachable by draws or by the literal scan
// when the source names no such constant. Fixed corners at the float64
// cliffs reach it, and they must name the exact witness.
#[test]
fn int_to_float_precision_divergence_above_2_53_is_caught() {
    let cand = tmp("gen_big_cand.py", "def double(n):\n    return int(n * 2.0)\n");
    let refr = tmp("gen_big_ref.py", "def double(n):\n    return n * 2\n");
    let r = review(&cand, &refr, &spec("double", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => assert!(
            input.contains("9007199254740993") || input.contains("-9007199254740993"),
            "expected 2^53 + 1 (or its negative) as the counterexample, got {input}"
        ),
        v => panic!("large int corner should catch this, got {v:?}"),
    }
}

// Holdout 083 shape: abs via math.fabs clears the last bit above 2^53.
#[test]
fn abs_via_fabs_divergence_is_caught() {
    let cand = tmp("gen_fabs_cand.py", "import math\n\ndef m(n):\n    return int(math.fabs(n))\n");
    let refr = tmp("gen_fabs_ref.py", "def m(n):\n    return abs(n)\n");
    let r = review(&cand, &refr, &spec("m", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
}

// The corners must not raise a false alarm on an exact rewrite at any
// magnitude: Python ints are arbitrary precision, so `not n & 1` and
// `n % 2 == 0` agree everywhere, including at i64::MIN.
#[test]
fn large_int_corners_cause_no_false_alarm_on_exact_rewrites() {
    let cand = tmp("gen_even_cand.py", "def e(n):\n    return not n & 1\n");
    let refr = tmp("gen_even_ref.py", "def e(n):\n    return n % 2 == 0\n");
    let r = review(&cand, &refr, &spec("e", vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    let cand = tmp("gen_sq_cand.py", "def q(a, b):\n    return a * a + b\n");
    let refr = tmp("gen_sq_ref.py", "def q(a, b):\n    return a ** 2 + b\n");
    let r = review(&cand, &refr, &spec("q", vec![ArgType::Int, ArgType::Int])).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
}

// The large corners run AFTER the random cases. A function whose work grows
// with n exhausts the step budget on a 2^63 input; placing those inputs last
// means a counterexample the small cases find still stands, and a run that
// is fine inside the envelope ends with the same verdict it had before the
// corners existed if it never reaches them.
#[test]
fn large_int_corners_run_after_the_random_cases() {
    let cand = tmp("gen_ord_cand.py", "def f(n):\n    t = 0\n    for i in range(abs(n) % 1000):\n        t += i\n    return t + (1 if n % 7 == 3 else 0)\n");
    let refr = tmp("gen_ord_ref.py", "def f(n):\n    t = 0\n    for i in range(abs(n) % 1000):\n        t += i\n    return t\n");
    let r = review(&cand, &refr, &spec("f", vec![ArgType::Int])).expect("no safety stop");
    match &r.verdict {
        ReviewVerdict::Counterexample { input, .. } => {
            let n: i64 = input.trim_matches(|c| c == '(' || c == ')' || c == ',').trim().parse().expect("int input");
            assert!(n.abs() <= 1_000_000, "the small cases should find this first, got {input}");
        }
        v => panic!("expected a counterexample from the small cases, got {v:?}"),
    }
}
