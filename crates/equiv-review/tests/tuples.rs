//! Constraint tests for tuple and namedtuple RETURN values. Written before the
//! implementation. Properties:
//! - a tuple is compared by value, element by element, order observable;
//! - a tuple is NOT a list: `(1, 2)` -> `[1, 2]` is a behaviour change;
//! - a one-element tuple is not its element: `(1,)` -> `1` is a change;
//! - a namedtuple is compared by class name, field names and values, so two
//!   separately defined classes with the same shape compare equal (candidate
//!   and reference each define their own), but a namedtuple -> plain tuple
//!   change is caught (attribute access is observable);
//! - anything inside a tuple that is not comparable (a set) is still refused,
//!   and other tuple subclasses are still refused. Never judge what we cannot
//!   compare.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn wp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_tuptest_{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}
fn spec(func: &str) -> ReviewSpec {
    ReviewSpec { func: func.into(), args: vec![ArgType::Int], n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET }
}
fn verdict(c: &str, r: &str, name: &str) -> ReviewVerdict {
    let cand = wp(&format!("{name}_c"), c);
    let refr = wp(&format!("{name}_r"), r);
    review(&cand, &refr, &spec("f")).expect("no safety stop").verdict
}

#[test]
fn same_tuple_is_equivalent_and_reproducible() {
    let cand = wp("eq_c", "def f(n):\n    return (n, n * 2)\n");
    let refr = wp("eq_r", "def f(n):\n    a = n\n    return a, a + a\n");
    let r1 = review(&cand, &refr, &spec("f")).expect("no safety stop");
    let r2 = review(&cand, &refr, &spec("f")).expect("no safety stop");
    assert!(matches!(r1.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r1.verdict);
    assert_eq!(r1.sha256(), r2.sha256());
}

#[test]
fn tuple_content_divergence_caught() {
    let v = verdict("def f(n):\n    return (n, n)\n", "def f(n):\n    return (n, n + 1)\n", "tc");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn tuple_order_is_observable() {
    let v = verdict("def f(n):\n    return (n, n + 1)\n", "def f(n):\n    return (n + 1, n)\n", "to");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn tuple_vs_list_is_a_change() {
    let v = verdict("def f(n):\n    return [n, n + 1]\n", "def f(n):\n    return (n, n + 1)\n", "tl");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn one_tuple_vs_scalar_is_a_change() {
    let v = verdict("def f(n):\n    return n\n", "def f(n):\n    return (n,)\n", "one");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn empty_tuple_vs_empty_list_is_a_change() {
    let v = verdict("def f(n):\n    return []\n", "def f(n):\n    return ()\n", "empty");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn tuple_nested_in_list_and_dict_is_compared() {
    let v = verdict(
        "def f(n):\n    return [(n, 1), {'k': (n,)}]\n",
        "def f(n):\n    return [tuple([n, 1]), {'k': (n,)}]\n",
        "nest",
    );
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

const NT_A: &str = "from collections import namedtuple\nRGB = namedtuple('RGB', ['red', 'green', 'blue'])\n";
const NT_TYPING: &str = "from typing import NamedTuple\nclass RGB(NamedTuple):\n    red: int\n    green: int\n    blue: int\n";

#[test]
fn separately_defined_namedtuples_same_shape_equivalent() {
    let body = "def f(n):\n    return RGB(n, n + 1, n + 2)\n";
    let v = verdict(&format!("{NT_TYPING}{body}"), &format!("{NT_A}{body}"), "nt_eq");
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

#[test]
fn namedtuple_value_divergence_caught() {
    let v = verdict(
        &format!("{NT_A}def f(n):\n    return RGB(n, n, n)\n"),
        &format!("{NT_A}def f(n):\n    return RGB(n, n + 1, n)\n"),
        "nt_val",
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn namedtuple_vs_plain_tuple_is_a_change() {
    let v = verdict(
        "def f(n):\n    return (n, n + 1, n + 2)\n",
        &format!("{NT_A}def f(n):\n    return RGB(n, n + 1, n + 2)\n"),
        "nt_plain",
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn namedtuple_field_rename_is_a_change() {
    let other = "from collections import namedtuple\nRGB = namedtuple('RGB', ['r', 'g', 'b'])\n";
    let body = "def f(n):\n    return RGB(n, n + 1, n + 2)\n";
    let v = verdict(&format!("{other}{body}"), &format!("{NT_A}{body}"), "nt_fields");
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn tuple_containing_set_still_refused() {
    let p = wp("tset", "def f(n):\n    return (n, {n})\n");
    let v = review(&p, &p, &spec("f")).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Refused { .. }), "got {v:?}");
}

#[test]
fn other_tuple_subclass_still_refused() {
    let p = wp("tsub", "class T(tuple):\n    pass\ndef f(n):\n    return T((n, n))\n");
    let v = review(&p, &p, &spec("f")).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Refused { .. }), "got {v:?}");
}

// Cross-host regression guard: the CI matrix runs this on all three targets,
// proving the tuple and namedtuple canonical forms are byte-identical.
#[test]
fn golden_tuple_receipt_id_is_stable() {
    let cand = wp("gt_c", &format!("{NT_A}def f(n):\n    return [(n, (n,)), RGB(n, 0, ())]\n"));
    let refr = wp("gt_r", &format!("{NT_A}def f(n):\n    return [(n, (n + 1,)), RGB(n, 0, ())]\n"));
    // python field pinned to the CI minor before hashing; see floats.rs.
    let mut r = review(&cand, &refr, &spec("f")).expect("no safety stop");
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
    r.python = "3.12".into();
    let id: String = r.sha256().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(id, "2575809de1c30eeeb2244f66f8a4c613347bbd4a92a7ad70ea40b2b95cb2dc97", "receipt-id drifted");
}
