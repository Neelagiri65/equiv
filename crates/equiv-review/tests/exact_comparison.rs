//! Founding-principle constraint test (P5): the default comparison is exact.
//! Every observable difference in a result is behaviour: its type, its exact
//! float bits, its order, and whether (and with which exception type) the
//! function raised. Python's `==` would call 42 and 42.0, or True and 1, the
//! same; a reviewer relying on that would wave through exactly the int->float
//! and bool->int regressions seen in real ports. Any tolerance must be opt-in
//! and recorded in the receipt; none exists today.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn write_py(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_p5_{name}.py"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    // Exhaustive literal on purpose: adding a tolerance/approximate-equality
    // field to ReviewSpec breaks this test and forces the P5 decision to be
    // made explicitly (opt-in, default exact, written into the receipt).
    ReviewSpec { func: func.into(), args, n: 200, seed: 1 }
}

fn diverges(name: &str, cand: &str, refr: &str, args: Vec<ArgType>, func: &str) {
    let c = write_py(&format!("{name}_c"), cand);
    let r = write_py(&format!("{name}_r"), refr);
    let v = review(&c, &r, &spec(func, args)).verdict;
    assert!(
        matches!(v, ReviewVerdict::Counterexample { .. }),
        "{name}: exact comparison must report a divergence, got {v:?}"
    );
}

// 42 == 42.0 in Python, but a typed client sees int vs float.
#[test]
fn int_result_becoming_float_diverges() {
    diverges("intfloat", "def f(x):\n    return x * 1.0\n", "def f(x):\n    return x\n", vec![ArgType::Int], "f");
}

// True == 1 in Python, but the type is observable.
#[test]
fn bool_result_becoming_int_diverges() {
    diverges(
        "boolint",
        "def f(x):\n    return 1 if x > 0 else 0\n",
        "def f(x):\n    return x > 0\n",
        vec![ArgType::Int],
        "f",
    );
}

// No epsilon: 0.1 + 0.2 is not 0.3.
#[test]
fn float_near_miss_diverges() {
    diverges(
        "nearmiss",
        "def f(x):\n    return x + 0.1 + 0.2\n",
        "def f(x):\n    return x + 0.3\n",
        vec![ArgType::Float],
        "f",
    );
}

// Raising is behaviour, and so is which exception.
#[test]
fn different_exception_type_diverges() {
    diverges(
        "exctype",
        "def f(x):\n    if x < 0:\n        raise TypeError('neg')\n    return x\n",
        "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x\n",
        vec![ArgType::Int],
        "f",
    );
}

#[test]
fn raise_versus_return_diverges() {
    diverges(
        "raiseret",
        "def f(x):\n    if x < 0:\n        return None\n    return x\n",
        "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x\n",
        vec![ArgType::Int],
        "f",
    );
}

// List order is observable.
#[test]
fn list_order_diverges() {
    diverges("order", "def f(x):\n    return [x + 1, x]\n", "def f(x):\n    return [x, x + 1]\n", vec![ArgType::Int], "f");
}

// Exactness must not over-fire: a genuine refactor is still equivalent.
#[test]
fn exact_refactor_still_equivalent() {
    let c = write_py("same_c", "def f(x):\n    if x < 0:\n        raise ValueError('a')\n    return [x * 2, x]\n");
    let r = write_py("same_r", "def f(x):\n    if x < 0:\n        raise ValueError('b')\n    return [x + x, x]\n");
    let v = review(&c, &r, &spec("f", vec![ArgType::Int])).verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}
