//! CLI constraint tests for the wall-clock safety stop (backlog 0.3b): what a
//! user actually sees. A safety stop prints could-not-check, exits 2, and emits
//! no receipt, no signed receipt and no attestation, even when a signing key is
//! configured. Other functions in the same `review-pr` run keep theirs.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const TERMINATES: &str = "def f(x):\n    return x\n";
const C_LEVEL_LONG: &str = "def f(x):\n    return sum(range(10**12))\n";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_safety_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn no_receipt_lines(stdout: &str) {
    for l in stdout.lines() {
        let l = l.trim_start();
        assert!(
            !(l.starts_with("receipt-id:") || l.starts_with("receipt:") || l.starts_with("signed-receipt:")),
            "a safety stop must not print receipt bytes: {stdout}"
        );
    }
}

#[test]
fn review_safety_stop_prints_no_receipt_even_with_sign() {
    let d = dir("review");
    std::fs::write(d.join("cand.py"), C_LEVEL_LONG).unwrap();
    std::fs::write(d.join("ref.py"), TERMINATES).unwrap();
    let out = Command::new(BIN)
        .args(["review", "cand.py", "ref.py", "f", "int", "--sign"])
        .current_dir(&d)
        .env("EQUIV_SAFETY_SECONDS", "2")
        .env("EQUIV_SIGNING_KEY", KEY)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "could not check: {stdout}");
    assert!(stdout.contains("not part of the deterministic budget"), "{stdout}");
    no_receipt_lines(&stdout);
}

#[test]
fn bad_safety_limit_is_invalid_input() {
    for bad in ["0", "abc", "-5"] {
        let out = Command::new(BIN)
            .args(["review", "a.py", "b.py", "f", "int"])
            .env("EQUIV_SAFETY_SECONDS", bad)
            .env("EQUIV_METRICS", "off")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3), "EQUIV_SAFETY_SECONDS={bad} must be rejected");
    }
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

#[test]
fn review_pr_safety_stop_is_not_signed_or_attested() {
    let d = dir("pr");
    std::fs::write(d.join("slow.py"), TERMINATES).unwrap();
    std::fs::write(d.join("fast.py"), TERMINATES).unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("slow.py"), C_LEVEL_LONG).unwrap();
    std::fs::write(d.join("fast.py"), "def f(x):\n    return x * 1\n").unwrap();
    std::fs::write(d.join("manifest"), "slow.py : f : int\nfast.py : f : int\n").unwrap();

    let out = Command::new(BIN)
        .args(["review-pr", "manifest", "--base", "HEAD", "--receipts-out", "receipts.tsv", "--attest-out", "att"])
        .current_dir(&d)
        .env("EQUIV_SAFETY_SECONDS", "2")
        .env("EQUIV_SIGNING_KEY", KEY)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("safety limit"), "{stdout}");

    // Both manifest entries name function `f`; the signed TSV and attestation
    // files must hold exactly one entry: the fast one.
    let tsv = std::fs::read_to_string(d.join("receipts.tsv")).unwrap();
    assert_eq!(tsv.lines().count(), 1, "only the fast function is signed: {tsv}");
    let atts: Vec<_> = std::fs::read_dir(d.join("att")).unwrap().collect();
    assert_eq!(atts.len(), 1, "only the fast function is attested");
    let stmt = std::fs::read_to_string(atts[0].as_ref().unwrap().path()).unwrap();
    assert!(stmt.contains("fast.py"), "{stmt}");
    assert!(!stmt.contains("slow.py"), "{stmt}");
}
