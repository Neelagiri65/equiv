//! `--same-host` on review and review-pr (0.26.0): the opt-in same-host float
//! tier, said loudly in the output. Written before the CLI change (holdout).
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_sh_{name}_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
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

const OPAQUE: &str = "def _h(x):\n    return x * 2.0\n\ndef f(x):\n    return _h(x)\n";

fn run(d: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(BIN).args(args).current_dir(d).env("EQUIV_METRICS", "off").output().unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn review_same_host_judges_and_says_so() {
    let d = dir("review");
    std::fs::write(d.join("c.py"), OPAQUE).unwrap();
    std::fs::write(d.join("r.py"), "def f(x):\n    return x + x\n").unwrap();
    let (plain, out) = run(&d, &["review", "c.py", "r.py", "f", "float"]);
    assert_eq!(plain, Some(2), "refused by default: {out}");
    let (code, out) = run(&d, &["review", "c.py", "r.py", "f", "float", "--same-host"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.lines().any(|l| l.starts_with("tier:") && l.contains("same-host")), "{out}");
}

#[test]
fn review_pr_same_host() {
    let d = dir("pr");
    std::fs::write(d.join("m.py"), "def f(x):\n    return x + x\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), OPAQUE).unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : float\n").unwrap();
    let (plain, out) = run(&d, &["review-pr", ".equiv-review", "--base", "HEAD"]);
    assert_eq!(plain, Some(2), "{out}");
    let (code, out) = run(&d, &["review-pr", ".equiv-review", "--base", "HEAD", "--same-host"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("SAME HOST ONLY"), "the PR comment says it: {out}");
}
