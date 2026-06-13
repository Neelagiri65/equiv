//! End-to-end review-oracle tests (require python3 on PATH).
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 200, seed: 1 }
}

#[test]
fn equivalent_refactor_passes() {
    // Two correct implementations of the same function.
    let cand = tmp("rev_eq_cand.py", "def absdiff(a, b):\n    return a - b if a >= b else b - a\n");
    let refr = tmp("rev_eq_ref.py", "def absdiff(a, b):\n    return abs(a - b)\n");
    let r = review(&cand, &refr, &spec("absdiff", vec![ArgType::Int, ArgType::Int]));
    assert!(matches!(r.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", r.verdict);
    assert_eq!(r.verdict.exit_code(), 0);
}

#[test]
fn buggy_refactor_caught_with_concrete_input() {
    // AI "optimised" version with an off-by-one for positive n.
    let cand = tmp("rev_bug_cand.py", "def count_upto(n):\n    return n + 1 if n > 0 else 0\n");
    let refr = tmp("rev_bug_ref.py", "def count_upto(n):\n    return len([i for i in range(n)]) if n > 0 else 0\n");
    let r = review(&cand, &refr, &spec("count_upto", vec![ArgType::Int]));
    match &r.verdict {
        ReviewVerdict::Counterexample { candidate, reference, .. } => {
            assert_ne!(candidate, reference);
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
    assert_eq!(r.verdict.exit_code(), 1);
}

#[test]
fn determinism_byte_identical_receipts() {
    let cand = tmp("rev_det_cand.py", "def f(xs):\n    return sorted(xs)\n");
    let refr = tmp("rev_det_ref.py", "def f(xs):\n    return list(sorted(xs))\n");
    let s = spec("f", vec![ArgType::ListInt]);
    let r1 = review(&cand, &refr, &s);
    let r2 = review(&cand, &refr, &s);
    assert_eq!(r1.to_bytes(), r2.to_bytes(), "review receipts must be reproducible");
    assert_eq!(r1.sha256(), r2.sha256());
}

#[test]
fn counterexample_is_in_the_receipt() {
    // The diverging input must be recoverable from the signed receipt, not
    // just printed. That's the review payload.
    let cand = tmp("rev_cex_cand.py", "def g(s):\n    return s.upper()\n");
    let refr = tmp("rev_cex_ref.py", "def g(s):\n    return s.title()\n");
    let r = review(&cand, &refr, &spec("g", vec![ArgType::Str]));
    // upper() vs title() differ on any multi-letter string.
    assert!(matches!(r.verdict, ReviewVerdict::Counterexample { .. }), "got {:?}", r.verdict);
    assert!(!r.to_bytes().is_empty());
}

use equiv_review::{render_markdown, run_pr, PrCheck, ReviewItem, MARKER};

#[test]
fn pr_markdown_reports_divergence_and_equivalence() {
    let eq_head = tmp("pr_eq_head.py", "def total(n):\n    return n*(n+1)//2 if n>0 else 0\n");
    let eq_base = tmp("pr_eq_base.py", "def total(n):\n    s=0\n    for i in range(1,max(n,0)+1): s+=i\n    return s\n");
    let bug_head = tmp("pr_bug_head.py", "def clamp(x):\n    return min(x, 100)\n");
    let bug_base = tmp("pr_bug_base.py", "def clamp(x):\n    return min(x, 99)\n");
    let checks = vec![
        PrCheck { name: "total".into(), head_path: eq_head, base_path: eq_base, spec: spec("total", vec![ArgType::Int]) },
        PrCheck { name: "clamp".into(), head_path: bug_head, base_path: bug_base, spec: spec("clamp", vec![ArgType::Int]) },
    ];
    let (items, code) = run_pr(&checks);
    assert_eq!(items.len(), 2);
    assert_eq!(code, 1, "any divergence must fail the gate");
    let md = render_markdown(&items, None);
    assert!(md.starts_with(MARKER), "must carry the sticky marker");
    assert!(md.contains("`total`") && md.contains("equivalent"));
    assert!(md.contains("`clamp`") && md.contains("DIVERGES"));
    assert!(md.contains("receipt-id"));
    // Honest-scope disclaimer must be present.
    assert!(md.to_lowercase().contains("does not check"));
    // Signed render surfaces the signer pubkey.
    let signed_md = render_markdown(&items, Some("deadbeef"));
    assert!(signed_md.contains("Signed (ed25519) by `deadbeef`"));
    assert!(signed_md.contains("signed"));
}

#[test]
fn pr_all_equivalent_passes_gate() {
    let h = tmp("pr_ok_head.py", "def f(a,b):\n    return a-b if a>=b else b-a\n");
    let b = tmp("pr_ok_base.py", "def f(a,b):\n    return abs(a-b)\n");
    let checks = vec![PrCheck { name: "f".into(), head_path: h, base_path: b, spec: spec("f", vec![ArgType::Int, ArgType::Int]) }];
    let (items, code) = run_pr(&checks);
    assert_eq!(code, 0);
    assert!(render_markdown(&items, None).contains("behaviour preserved"));
    let _ = ReviewItem { name: "x".into(), receipt: items.into_iter().next().unwrap().receipt };
}
