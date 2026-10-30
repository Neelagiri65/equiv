//! Backlog 0.3c: "could not check" is never shown as green and is reported
//! separately from a divergence, so a gate can tell the two apart.
//!
//! The exit code from `run_pr` stays a plain report: 0 every function checked
//! and equivalent, 1 any divergence, 2 nothing diverged but something was not
//! checked. Which of those blocks a merge is the wrapper's policy (the Action's
//! `fail-on-divergence` / `fail-on-not-checked` inputs), tested on the CLI side.
use equiv_review::{render_markdown, render_markdown_with_unchecked, run_pr_limited, ArgType, PrCheck, ReviewSpec};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_ncg_{}_{name}.py", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

/// The first line of the comment body: what a reader sees before anything else.
fn headline(md: &str) -> String {
    md.lines().skip_while(|l| !l.starts_with("## equiv review")).skip(1).find(|l| !l.trim().is_empty()).unwrap().to_string()
}

fn spec(func: &str, args: Vec<ArgType>) -> ReviewSpec {
    ReviewSpec { func: func.into(), args, n: 50, seed: 1, budget: equiv_review::DEFAULT_BUDGET }
}

/// A safety stop next to an equivalent function: exit 2 (not 1), and the
/// headline says how many were not checked and never says "preserved".
#[test]
fn not_checked_is_exit_2_and_never_reads_green() {
    let slow_head = tmp("slow_h", "def f(x):\n    return sum(range(10**12))\n");
    let slow_base = tmp("slow_b", "def f(x):\n    return x\n");
    let fast_head = tmp("fast_h", "def f(x):\n    return x * 1\n");
    let fast_base = tmp("fast_b", "def f(x):\n    return x\n");
    let checks = vec![
        PrCheck { name: "slow".into(), head_path: slow_head, base_path: slow_base, spec: spec("f", vec![ArgType::Int]) },
        PrCheck { name: "fast".into(), head_path: fast_head, base_path: fast_base, spec: spec("f", vec![ArgType::Int]) },
    ];
    let (items, code) = run_pr_limited(&checks, 3);
    assert_eq!(code, 2, "not checked is reported as 2, distinct from a divergence (1)");
    let md = render_markdown(&items, None);
    let h = headline(&md);
    assert!(h.contains("1 of 2 functions could not be checked"), "{h}");
    assert!(!h.to_lowercase().contains("preserved"), "a run with an unchecked function must not read green:\n{md}");
}

/// Auto-detect found changed functions but could check none of them: the
/// headline must say nothing was checked, never "behaviour preserved".
#[test]
fn nothing_checked_is_not_reported_as_preserved() {
    let unchecked = vec![
        ("g".to_string(), "no supported type hint on every parameter".to_string()),
        ("h".to_string(), "method, not checked (module-level functions only)".to_string()),
    ];
    let md = render_markdown_with_unchecked(&[], None, &unchecked);
    let h = headline(&md);
    assert!(!h.to_lowercase().contains("preserved"), "{md}");
    assert!(h.contains("Nothing was checked") && h.contains("2 changed functions"), "{h}");
    assert!(md.contains("changed, not checked"), "{md}");
    assert!(!md.contains("| function | result | detail |"), "no empty result table when nothing ran:\n{md}");
}

/// Checked-and-equivalent functions next to unchecked ones: the headline must
/// carry the unchecked count, not only the section further down.
#[test]
fn headline_counts_unchecked_next_to_equivalent() {
    let h = tmp("ok_h", "def f(a):\n    return a + 0\n");
    let b = tmp("ok_b", "def f(a):\n    return a\n");
    let checks = vec![PrCheck { name: "f".into(), head_path: h, base_path: b, spec: spec("f", vec![ArgType::Int]) }];
    let (items, code) = run_pr_limited(&checks, 30);
    assert_eq!(code, 0);
    let unchecked = vec![("g".to_string(), "async function, not checked in this version".to_string())];
    let md = render_markdown_with_unchecked(&items, None, &unchecked);
    let h = headline(&md);
    assert!(h.contains("1 changed function was not checked"), "{h}");
}
