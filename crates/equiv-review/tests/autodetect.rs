//! Constraint tests for auto-detection of changed functions (closes the
//! manifest-bypass, issue #1). The invariant under test: a changed function is
//! NEVER silently dropped. It is either Checkable or surfaced as NotChecked.
//! See docs/spec-auto-detect-changed-fns.md.

use equiv_review::{detect_changed_functions, render_markdown_with_unchecked, ArgType, Detected, DetectedStatus};

fn find<'a>(v: &'a [Detected], name: &str) -> Option<&'a Detected> {
    v.iter().find(|d| d.name == name)
}

#[test]
fn changed_hinted_function_is_checkable() {
    let d = detect_changed_functions("def f(x: int):\n    return x\n", "def f(x: int):\n    return x + 1\n").unwrap();
    match &find(&d, "f").expect("f present").status {
        DetectedStatus::Checkable { args } => assert_eq!(args, &vec![ArgType::Int]),
        s => panic!("expected Checkable, got {s:?}"),
    }
}

// THE soundness test: a changed function with no hints must be surfaced, not
// silently dropped (that was the manifest-bypass bug).
#[test]
fn changed_unhinted_function_is_surfaced_not_dropped() {
    let d = detect_changed_functions("def f(x):\n    return x\n", "def f(x):\n    return x + 1\n").unwrap();
    let f = find(&d, "f").expect("a changed function must never be silently dropped");
    assert!(matches!(f.status, DetectedStatus::NotChecked { .. }), "got {:?}", f.status);
}

#[test]
fn unsupported_hint_is_not_checked() {
    let d = detect_changed_functions("def f(x: complex):\n    return x\n", "def f(x: complex):\n    return x + 1\n").unwrap();
    assert!(matches!(find(&d, "f").unwrap().status, DetectedStatus::NotChecked { .. }));
}

#[test]
fn added_function_reported_as_new() {
    let base = "def f(x: int):\n    return x\n";
    let head = "def f(x: int):\n    return x\ndef g(y: int):\n    return y\n";
    let d = detect_changed_functions(base, head).unwrap();
    assert!(find(&d, "f").is_none(), "unchanged f must be absent");
    match &find(&d, "g").expect("new g must be reported").status {
        DetectedStatus::NotChecked { reason } => assert!(reason.contains("new"), "reason: {reason}"),
        s => panic!("{s:?}"),
    }
}

#[test]
fn removed_function_reported() {
    let base = "def f(x: int):\n    return x\ndef g(y: int):\n    return y\n";
    let head = "def f(x: int):\n    return x\n";
    assert!(matches!(find(&detect_changed_functions(base, head).unwrap(), "g").unwrap().status, DetectedStatus::NotChecked { .. }));
}

#[test]
fn arity_change_is_not_checked() {
    let d = detect_changed_functions("def f(x: int):\n    return x\n", "def f(x: int, y: int):\n    return x + y\n").unwrap();
    assert!(matches!(find(&d, "f").unwrap().status, DetectedStatus::NotChecked { .. }));
}

#[test]
fn unchanged_function_is_absent() {
    assert!(detect_changed_functions("def f(x: int):\n    return x + 1\n", "def f(x: int):\n    return x + 1\n").unwrap().is_empty());
}

#[test]
fn comment_only_change_is_absent() {
    // The AST ignores comments. A comment-only edit is not a behaviour change.
    let d = detect_changed_functions("def f(x: int):\n    return x + 1\n", "def f(x: int):\n    # note\n    return x + 1\n").unwrap();
    assert!(d.is_empty());
}

#[test]
fn multi_type_signature_is_inferred() {
    let base = "def f(a: int, s: str, xs: list[int], d: dict, z: float):\n    return a\n";
    let head = "def f(a: int, s: str, xs: list[int], d: dict, z: float):\n    return a + 1\n";
    match &find(&detect_changed_functions(base, head).unwrap(), "f").unwrap().status {
        DetectedStatus::Checkable { args } => {
            assert_eq!(args, &vec![ArgType::Int, ArgType::Str, ArgType::ListInt, ArgType::Dict, ArgType::Float])
        }
        s => panic!("{s:?}"),
    }
}

#[test]
fn not_checked_section_is_rendered() {
    let unchecked = vec![("f".to_string(), "no type hints".to_string())];
    let md = render_markdown_with_unchecked(&[], None, &unchecked);
    assert!(md.contains("changed, not checked"));
    assert!(md.contains("`f`"));
    assert!(md.contains("does not cover"));
}

// Prosecution fix F1: a changed async function must be surfaced, not invisible.
#[test]
fn async_changed_function_is_surfaced() {
    let d = detect_changed_functions(
        "async def f(x: int):\n    return x\n",
        "async def f(x: int):\n    return x + 1\n",
    ).unwrap();
    match &find(&d, "f").expect("async fn must be surfaced, not invisible").status {
        DetectedStatus::NotChecked { reason } => assert!(reason.contains("async"), "reason: {reason}"),
        s => panic!("{s:?}"),
    }
}

// Prosecution fix F2: a changed method must be surfaced (qualified name), not
// silently dropped.
#[test]
fn method_changed_is_surfaced() {
    let base = "class C:\n    def m(self, x: int):\n        return x\n";
    let head = "class C:\n    def m(self, x: int):\n        return x + 1\n";
    let d = detect_changed_functions(base, head).unwrap();
    let m = find(&d, "C.m").expect("method must be surfaced");
    assert!(matches!(m.status, DetectedStatus::NotChecked { .. }), "got {:?}", m.status);
}

// Prosecution fix F2: a changed nested function must be surfaced.
#[test]
fn nested_changed_function_is_surfaced() {
    let base = "def outer(x: int):\n    def inner(y: int):\n        return y\n    return inner(x)\n";
    let head = "def outer(x: int):\n    def inner(y: int):\n        return y + 1\n    return inner(x)\n";
    let d = detect_changed_functions(base, head).unwrap();
    let inner = find(&d, "outer.inner").expect("nested fn must be surfaced");
    assert!(matches!(inner.status, DetectedStatus::NotChecked { .. }));
}

// A head file that does not parse is a whole-run error, never an empty list:
// an empty list would render as "0 checked functions: behaviour preserved",
// the gate-level false pass the invariant above forbids.
#[test]
fn unparseable_head_is_an_error_not_an_empty_list() {
    let err = detect_changed_functions("def f(x: int):\n    return x\n", "def f(x: int:\n    return x\n")
        .expect_err("a syntax-broken head must not detect as 'nothing changed'");
    assert!(err.contains("head source could not be parsed"), "{err}");
    let err = detect_changed_functions("", "def f(:\n").expect_err("no base, broken head");
    assert!(err.contains("could not be parsed"), "{err}");
}

// Backlog 1.2, gap 4: positional defaults no longer hide a function. Every
// parameter, defaulted or not, must carry a supported hint, and every
// argument is generated (the omitted-argument path is then the same default
// on both sides). A changed default is a behaviour change the generated
// inputs cannot see, so it is reported, not checked.
#[test]
fn hinted_function_with_a_default_is_checkable_on_its_full_signature() {
    let base = "def f(n: int, flag: bool = False):\n    return n\n";
    let head = "def f(n: int, flag: bool = False):\n    return n + 0\n";
    let v = detect_changed_functions(base, head).unwrap();
    match &find(&v, "f").expect("f detected").status {
        DetectedStatus::Checkable { args } => assert_eq!(args, &vec![ArgType::Int, ArgType::Bool]),
        s => panic!("{s:?}"),
    }
}

#[test]
fn changed_default_value_is_reported_not_checked() {
    let base = "def f(n: int, k: int = 3):\n    return n * k\n";
    let head = "def f(n: int, k: int = 4):\n    return n * k\n";
    let v = detect_changed_functions(base, head).unwrap();
    match &find(&v, "f").expect("f detected").status {
        DetectedStatus::NotChecked { reason } => assert!(reason.contains("default"), "{reason}"),
        s => panic!("{s:?}"),
    }
}

#[test]
fn keyword_only_and_star_args_are_still_not_checked() {
    let base = "def f(n: int, *, k: int = 3):\n    return n * k\n\ndef g(*xs: int):\n    return 0\n";
    let head = "def f(n: int, *, k: int = 3):\n    return n + n * k\n\ndef g(*xs: int):\n    return 1\n";
    let v = detect_changed_functions(base, head).unwrap();
    for name in ["f", "g"] {
        assert!(matches!(find(&v, name).unwrap().status, DetectedStatus::NotChecked { .. }), "{name}");
    }
}

#[test]
fn bool_and_list_str_hints_are_detected() {
    let base = "def f(xs: list[str], b: bool) -> int:\n    return len(xs)\n";
    let head = "def f(xs: list[str], b: bool) -> int:\n    return len(xs) + 0\n";
    let v = detect_changed_functions(base, head).unwrap();
    match &find(&v, "f").unwrap().status {
        DetectedStatus::Checkable { args } => assert_eq!(args, &vec![ArgType::ListStr, ArgType::Bool]),
        s => panic!("{s:?}"),
    }
}
