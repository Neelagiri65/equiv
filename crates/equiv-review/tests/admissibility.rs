//! Admissibility on every signature (backlog 0.5c). The README says a function
//! that reaches a transcendental is refused by name; until this change that
//! held for float signatures only, and an int, str or list review of code
//! whose result rests on a maths-library rounding returned a signed verdict.
//! The receipt was honest about its inputs, but not reproducible across maths
//! libraries. These tests pin: libm by name is refused under every signature,
//! exact integer functions stay admitted, and int ** int stays admitted.
//! (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_adm{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false }
}

fn refused_naming(cand: &str, refr: &str, func: &str, args: Vec<ArgType>, name: &str) {
    let c = tmp(&format!("{func}_c"), cand);
    let r = tmp(&format!("{func}_r"), refr);
    let v = review(&c, &r, &spec(func, args)).expect("no safety stop").verdict;
    match v {
        ReviewVerdict::Refused { reason } => assert!(reason.contains(name), "reason should name {name}: {reason}"),
        v => panic!("{func} reaches {name} and must be refused by name, got {v:?}"),
    }
}

// Blind case b11's shape: an int signature reaching math.log2.
#[test]
fn int_signature_reaching_log2_is_refused_by_name() {
    refused_naming(
        "import math\n\ndef is_pow2(n):\n    return n > 0 and math.log2(n).is_integer()\n",
        "def is_pow2(n):\n    return n > 0 and (n & (n - 1)) == 0\n",
        "is_pow2",
        vec![ArgType::Int],
        "log2",
    );
}

// The guard follows the import, not the spelling.
#[test]
fn libm_reached_through_an_alias_or_from_import_is_refused() {
    refused_naming(
        "from math import log10 as lg\n\ndef d(n):\n    return 1 if n == 0 else int(lg(abs(n))) + 1\n",
        "def d(n):\n    return len(str(abs(n)))\n",
        "d",
        vec![ArgType::Int],
        "log10",
    );
    refused_naming(
        "import math as m\n\ndef e(n):\n    return int(m.exp(n))\n",
        "def e(n):\n    return 1\n",
        "e",
        vec![ArgType::Int],
        "exp",
    );
}

// Holdout 088's shape: the guard is per function body, whatever the types.
#[test]
fn list_and_str_signatures_are_guarded_too() {
    refused_naming(
        "import math\n\ndef bl(xs):\n    return [int(math.log2(x)) + 1 for x in xs]\n",
        "def bl(xs):\n    return [x.bit_length() for x in xs]\n",
        "bl",
        vec![ArgType::ListInt],
        "log2",
    );
    refused_naming(
        "import math\n\ndef w(s):\n    return int(math.sin(len(s)) * 10)\n",
        "def w(s):\n    return len(s)\n",
        "w",
        vec![ArgType::Str],
        "sin",
    );
}

// ** with a float operand is pow(3): a maths-library call in disguise.
#[test]
fn float_power_under_an_int_signature_is_refused() {
    refused_naming(
        "def r(n):\n    return round(abs(n) ** 0.5)\n",
        "import math\n\ndef r(n):\n    return math.isqrt(abs(n))\n",
        "r",
        vec![ArgType::Int],
        "**",
    );
}

// Exact integer functions and int ** int are not libm and stay admitted:
// gcd (holdout 087), isqrt, fabs (holdout 083 is caught through it) and a
// 2 ** 31 branch constant (the literal scan reads it from the source).
#[test]
fn exact_math_functions_and_int_power_stay_admitted() {
    let c = tmp("gcd_c", "import math\n\ndef gcd(a, b):\n    return math.gcd(a, b)\n");
    let r = tmp("gcd_r", "def gcd(a, b):\n    while b:\n        a, b = b, a % b\n    return abs(a)\n");
    let v = review(&c, &r, &spec("gcd", vec![ArgType::Int, ArgType::Int])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "gcd is exact and must be judged, got {v:?}");

    let c = tmp("isq_c", "import math\n\ndef q(n):\n    return math.isqrt(abs(n))\n");
    let r = tmp("isq_r", "import math\n\ndef q(n):\n    return math.isqrt(abs(n)) + (1 if n == 777 else 0)\n");
    let v = review(&c, &r, &spec("q", vec![ArgType::Int])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "isqrt is exact and must be judged, got {v:?}");

    let c = tmp("fabs_c", "import math\n\ndef m(n):\n    return int(math.fabs(n))\n");
    let r = tmp("fabs_r", "def m(n):\n    return abs(n)\n");
    let v = review(&c, &r, &spec("m", vec![ArgType::Int])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "fabs is exact; the 2^53 + 1 corner must still catch this, got {v:?}");

    let c = tmp("pow_c", "def big(n):\n    return 1 if n >= 2 ** 31 else 0\n");
    let r = tmp("pow_r", "def big(n):\n    return 0\n");
    let v = review(&c, &r, &spec("big", vec![ArgType::Int])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "int ** int is exact and must be judged, got {v:?}");
}
