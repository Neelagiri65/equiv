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
