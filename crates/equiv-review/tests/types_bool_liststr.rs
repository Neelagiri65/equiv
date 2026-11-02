//! `bool` and `list[str]` signatures (backlog 1.2, gap 6). A two-valued type
//! needs both values as fixed corners; a list of strings needs the string
//! corners lifted into lists plus the orderings that tie-breaks and sorts
//! care about. (require python3 on PATH)
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_ty{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET }
}

#[test]
fn bool_and_list_str_parse_and_hash_distinctly() {
    assert_eq!(ArgType::parse("bool"), Some(ArgType::Bool));
    assert_eq!(ArgType::parse("list[str]"), Some(ArgType::ListStr));
    assert_eq!(ArgType::parse("List[str]"), Some(ArgType::ListStr));
    let c = tmp("h_c", "def f(a, b):\n    return 0\n");
    let r = tmp("h_r", "def f(a, b):\n    return 0\n");
    let a = review(&c, &r, &spec("f", vec![ArgType::Int, ArgType::Bool])).expect("no safety stop");
    let b = review(&c, &r, &spec("f", vec![ArgType::Int, ArgType::ListStr])).expect("no safety stop");
    assert_ne!(a.spec_sha256, b.spec_sha256, "different signatures must hash differently");
}

// Holdout 089's shape: a flag whose two branches were swapped.
#[test]
fn bool_flag_divergence_is_caught() {
    let c = tmp("fl_c", "def pick(xs, first):\n    return xs[-1] if first else xs[0]\n");
    let r = tmp("fl_r", "def pick(xs, first):\n    return xs[0] if first else xs[-1]\n");
    let v = review(&c, &r, &spec("pick", vec![ArgType::ListInt, ArgType::Bool])).expect("no safety stop").verdict;
    // Any input where the two branches pick different elements; the list
    // corner [1, 2, 3] with the flag at its default already does.
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "{v:?}");
}

#[test]
fn bool_exact_rewrite_passes() {
    let c = tmp("be_c", "def g(n, neg):\n    return -n if neg else n\n");
    let r = tmp("be_r", "def g(n, neg):\n    return n * (-1 if neg else 1)\n");
    let v = review(&c, &r, &spec("g", vec![ArgType::Int, ArgType::Bool])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "{v:?}");
}

// Holdout 090's shape: max keeps the first of a tie, a stable sort the last.
#[test]
fn list_str_tiebreak_divergence_is_caught() {
    let c = tmp("lo_c", "def longest(xs):\n    return sorted(xs, key=len)[-1]\n");
    let r = tmp("lo_r", "def longest(xs):\n    return max(xs, key=len)\n");
    let v = review(&c, &r, &spec("longest", vec![ArgType::ListStr])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "{v:?}");
}

// A list[str] divergence that needs a string corner inside the list: lower()
// and casefold() only differ on characters the random a-z strings never hold.
#[test]
fn list_str_carries_the_string_corners() {
    let c = tmp("cf_c", "def norm(xs):\n    return [x.casefold() for x in xs]\n");
    let r = tmp("cf_r", "def norm(xs):\n    return [x.lower() for x in xs]\n");
    let v = review(&c, &r, &spec("norm", vec![ArgType::ListStr])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "{v:?}");
}

#[test]
fn list_str_exact_rewrite_passes() {
    let c = tmp("le_c", "def shout(xs):\n    return ','.join(xs).upper()\n");
    let r = tmp("le_r", "def shout(xs):\n    return ','.join(x.upper() for x in xs)\n");
    let v = review(&c, &r, &spec("shout", vec![ArgType::ListStr])).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "{v:?}");
}
