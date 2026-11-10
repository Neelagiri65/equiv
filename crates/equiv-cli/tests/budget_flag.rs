//! `--budget N` (sqlfluff replay, 2026-11-10): heavy functions such as SQL
//! parsers need more than the default step budget per call. The budget is
//! already bound into the receipt; this exposes it on `review` and `review-pr`.
//! Written before the change (holdout).
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_budget_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(d: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(d)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

// About 3M line/jump events at x == 5; cheap elsewhere.
const HEAVY: &str = "def f(x):\n    t = 0\n    if x == 5:\n        for i in range(1500000):\n            t += 1\n    return t + x\n";

fn run(d: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(BIN).args(args).current_dir(d).env("EQUIV_METRICS", "off").output().unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn review_takes_a_budget_and_binds_it() {
    let d = dir("review");
    std::fs::write(d.join("c.py"), HEAVY).unwrap();
    std::fs::write(d.join("r.py"), HEAVY).unwrap();
    let (small, out_s) = run(&d, &["review", "c.py", "r.py", "f", "int[0..9]", "--budget", "1000000"]);
    let (big, out_b) = run(&d, &["review", "c.py", "r.py", "f", "int[0..9]", "--budget", "50000000"]);
    // A small budget: x == 5 is a range corner inside the main cases, so
    // exhaustion there is could-not-check (exit 2), as before.
    assert_eq!(small, Some(2), "{out_s}");
    assert_eq!(big, Some(0), "{out_b}");
    let id = |s: &str| s.lines().find(|l| l.starts_with("receipt-id:")).map(str::to_string);
    let (_, out_b2) = run(&d, &["review", "c.py", "r.py", "f", "int[0..9]", "--budget", "50000001"]);
    assert_ne!(id(&out_b), id(&out_b2), "the budget is bound into the receipt");
}

#[test]
fn review_rejects_a_bad_budget() {
    let d = dir("bad");
    std::fs::write(d.join("c.py"), HEAVY).unwrap();
    std::fs::write(d.join("r.py"), HEAVY).unwrap();
    for bad in ["0", "-5", "lots"] {
        let (code, out) = run(&d, &["review", "c.py", "r.py", "f", "int", "--budget", bad]);
        assert!(code != Some(0) && code != Some(1), "{bad}: {out}");
        assert!(out.contains("--budget"), "{bad}: {out}");
    }
}

#[test]
fn review_pr_takes_a_budget() {
    let d = dir("pr");
    std::fs::write(d.join("m.py"), HEAVY).unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), HEAVY.replace("return t + x", "return x + t")).unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int[0..9]\n").unwrap();
    let (small, out_s) = run(&d, &["review-pr", ".equiv-review", "--base", "HEAD", "--budget", "1000000"]);
    let (big, out_b) = run(&d, &["review-pr", ".equiv-review", "--base", "HEAD", "--budget", "50000000"]);
    assert_eq!(small, Some(2), "{out_s}");
    assert_eq!(big, Some(0), "{out_b}");
}
