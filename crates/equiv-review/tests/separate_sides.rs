//! Each side runs in its own interpreter and must report every input.
//!
//! Holdouts written before the change. A review is only "equivalent" when
//! both sides reported an outcome for every generated input; one side cannot
//! see or alter what the other side receives, returns or has loaded; and a
//! value equiv cannot read is never counted as "both raised".

use equiv_review::{review, ArgType, Domain, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_sides_{}_{name}.py", std::process::id()));
    std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
    p
}

fn spec(args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: "f".into(), args, n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false }
}

fn verdict(name: &str, cand: &str, refr: &str, s: &ReviewSpec) -> ReviewVerdict {
    let c = tmp(&format!("{name}_c"), cand);
    let r = tmp(&format!("{name}_r"), refr);
    review(&c, &r, s).expect("no safety stop").verdict
}

fn not_green(v: &ReviewVerdict) -> bool {
    !matches!(v, ReviewVerdict::Equivalent { .. })
}

// ---- a run that ends early is never a pass ----

#[test]
fn exit_zero_inside_the_function_is_not_a_pass() {
    let v = verdict(
        "exit0",
        "import sys\ndef f(n):\n    if n == 0: sys.exit(0)\n    return n + 1\n",
        "def f(n):\n    return n\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(not_green(&v), "a run cut short by sys.exit(0) passed: {v:?}");
}

#[test]
fn hard_exit_inside_the_function_is_not_a_pass() {
    let v = verdict(
        "osexit",
        "import os\ndef f(n):\n    if n == 3: os._exit(0)\n    return n + 1\n",
        "def f(n):\n    return n\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Error { .. }), "expected could-not-check, got {v:?}");
}

#[test]
fn replacing_stdout_at_import_does_not_hide_a_difference() {
    let v = verdict(
        "devnull",
        "import sys, os\nsys.stdout = open(os.devnull, 'w')\ndef f(n):\n    return n + 1\n",
        "def f(n):\n    return n\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

// ---- the function's own prints are not result lines ----

#[test]
fn printing_result_shaped_lines_changes_nothing() {
    let body = "def f(n):\n    print('0\\tSKIP'); print('0\\tSKIP'); print('999999\\tNE\\ta\\tb')\n    return n\n";
    let v = verdict("prints", body, body, &spec(vec![ArgType::Int]));
    match v {
        ReviewVerdict::Equivalent { n, not_judged } => {
            assert!(n > 0 && n < 100_000, "n = {n}");
            assert_eq!(not_judged, 0);
        }
        v => panic!("a function that prints must still be judged on its return value, got {v:?}"),
    }
}

#[test]
fn printing_does_not_hide_a_difference() {
    let v = verdict(
        "prints_ne",
        "def f(n):\n    print(n)\n    return n + 1 if n == 7 else n\n",
        "def f(n):\n    print('x')\n    return n\n",
        &ReviewSpec { domains: vec![Domain::IntRange(0, 20)], ..spec(vec![ArgType::Int]) },
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

// ---- one side cannot change what the other side sees ----

#[test]
fn a_side_that_empties_its_argument_does_not_change_the_other_sides_input() {
    let v = verdict(
        "mutate",
        "def f(xs):\n    xs[:] = []\n    return 0\n",
        "def f(xs):\n    return len(xs)\n",
        &spec(vec![ArgType::ListInt]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn state_set_at_import_by_one_side_does_not_reach_the_other() {
    let v = verdict(
        "decimal",
        "import decimal\ndecimal.getcontext().prec = 3\ndef f(n):\n    return str(decimal.Decimal(n) / decimal.Decimal(7))\n",
        "import decimal\ndef f(n):\n    return str(decimal.Decimal(n) / decimal.Decimal(7))\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn a_side_cannot_swap_the_function_the_other_side_runs() {
    let v = verdict(
        "rebind",
        "def f(n):\n    import __main__\n    for k in ('rf', 'cf', 'fn', '_fn'):\n        if hasattr(__main__, k): setattr(__main__, k, lambda *a: 424242)\n    return 424242\n",
        "def f(n):\n    return n\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

// ---- a value equiv cannot read is not "both raised" ----

#[test]
fn very_large_ints_that_differ_are_a_difference() {
    let v = verdict(
        "bigint",
        "def f(n):\n    return n if abs(n) < 50 else 10**5000 + n\n",
        "def f(n):\n    return n if abs(n) < 50 else 10**5000 - n\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn very_large_ints_that_agree_are_equivalent_with_no_errors_counted() {
    let body = "def f(n):\n    return 10**5000 + n\n";
    let c = tmp("bigeq_c", body);
    let r = tmp("bigeq_r", body);
    let rec = review(&c, &r, &spec(vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
    assert_eq!(rec.exceptions, 0, "nothing raised; nothing may be counted as raised");
}

#[test]
fn a_self_containing_list_is_refused_not_counted_as_an_error() {
    let body = "def f(n):\n    xs = [n]\n    xs.append(xs)\n    return xs\n";
    let v = verdict("cycle", body, body, &spec(vec![ArgType::Int]));
    assert!(matches!(v, ReviewVerdict::Refused { .. }), "got {v:?}");
}

// ---- a subclass is not its base type ----

#[test]
fn a_counter_is_not_a_plain_dict() {
    let v = verdict(
        "counter",
        "from collections import Counter\ndef f(n):\n    return Counter({'a': n})\n",
        "def f(n):\n    return {'a': n}\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn a_str_subclass_is_not_a_plain_str() {
    let v = verdict(
        "strsub",
        "class S(str):\n    def upper(self): return 'x'\ndef f(n):\n    return S(n)\n",
        "def f(n):\n    return str(n)\n",
        &spec(vec![ArgType::Int]),
    );
    assert!(matches!(v, ReviewVerdict::Counterexample { .. }), "got {v:?}");
}

#[test]
fn the_same_subclass_on_both_sides_agrees() {
    let body = "from collections import Counter, OrderedDict\ndef f(n):\n    return [Counter({'a': n}), OrderedDict(b=n)]\n";
    let v = verdict("samesub", body, body, &spec(vec![ArgType::Int]));
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

// ---- a pass needs a returned value unless the input space is tiny ----

#[test]
fn a_handful_of_inputs_that_all_raise_is_not_a_pass() {
    // list[int[1..1]] is an unbounded space (any length); only a few distinct
    // lists are generated. Every one raises on both sides.
    let s = ReviewSpec { domains: vec![Domain::IntRange(1, 1)], ..spec(vec![ArgType::ListInt]) };
    let v = verdict(
        "thin",
        "def f(xs):\n    raise ValueError('x')\n",
        "def f(xs):\n    if len(xs) > 6: return 1\n    raise ValueError('y')\n",
        &s,
    );
    assert!(not_green(&v), "only errors were compared, yet it passed: {v:?}");
}

#[test]
fn a_bool_function_that_always_raises_is_still_checked_completely() {
    // Two possible inputs, both tried: the existing exemption stands.
    let body = "def f(b):\n    raise ValueError('no')\n";
    let v = verdict("boolraise", body, body, &spec(vec![ArgType::Bool]));
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

// ---- unchanged behaviour (correct rewrites stay green) ----

#[test]
fn a_correct_rewrite_that_prints_and_mutates_a_copy_is_equivalent() {
    let v = verdict(
        "okrewrite",
        "def f(xs):\n    ys = list(xs)\n    ys.sort()\n    print('sorted', len(ys))\n    return ys\n",
        "def f(xs):\n    return sorted(xs)\n",
        &spec(vec![ArgType::ListInt]),
    );
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

#[test]
fn same_error_class_on_both_sides_still_agrees_and_is_counted() {
    let c = tmp("err_c", "def f(n):\n    if n < 0: raise ValueError('negative')\n    return n\n");
    let r = tmp("err_r", "def f(n):\n    if n < 0: raise ValueError('below zero')\n    return n\n");
    let rec = review(&c, &r, &spec(vec![ArgType::Int])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
    assert!(rec.exceptions > 0);
}
