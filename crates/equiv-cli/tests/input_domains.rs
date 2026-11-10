//! Input domains through the CLI (0.23.0): `equiv review` and the PR manifest
//! (`file : fn : sig`, the Action path) both accept `int[lo..hi]`,
//! `str[ascii]`, `str[1]` and their list forms, and say so in the output.
//! Written before the CLI change (holdout).
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_dom_{name}_{}", std::process::id()));
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

const TWIN: &str = "def f(x):\n    if x < 1 or x > 9999:\n        raise ValueError('year out of range')\n    return x * 2\n";
const REFR: &str = "def f(x):\n    return x + x\n";

fn review(d: &Path, sig: &str) -> (Option<i32>, String) {
    std::fs::write(d.join("c.py"), TWIN).unwrap();
    std::fs::write(d.join("r.py"), REFR).unwrap();
    let out = Command::new(BIN)
        .args(["review", "c.py", "r.py", "f", sig])
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn review_accepts_a_domain_and_prints_it() {
    let d = dir("review");
    let (plain, out) = review(&d, "int");
    assert_eq!(plain, Some(1), "{out}");
    let (code, out) = review(&d, "int[1..9999]");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.lines().any(|l| l.starts_with("domains:") && l.contains("int[1..9999]")), "{out}");
}

#[test]
fn review_without_domains_prints_no_domain_line() {
    let d = dir("nodom");
    let (_, out) = review(&d, "int");
    assert!(!out.lines().any(|l| l.starts_with("domains:")), "{out}");
}

#[test]
fn review_rejects_a_malformed_domain() {
    let d = dir("bad");
    let (code, out) = review(&d, "int[9..1]");
    assert_ne!(code, Some(0), "{out}");
    assert_ne!(code, Some(1), "{out}");
    assert!(out.contains("int[9..1]"), "{out}");
}

#[test]
fn pr_manifest_accepts_a_domain() {
    let d = dir("pr");
    std::fs::write(d.join("m.py"), REFR).unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), TWIN).unwrap();
    let run = |manifest: &str| {
        std::fs::write(d.join(".equiv-review"), manifest).unwrap();
        let out = Command::new(BIN)
            .args(["review-pr", ".equiv-review", "--base", "HEAD"])
            .current_dir(&d)
            .env("EQUIV_METRICS", "off")
            .output()
            .unwrap();
        (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let (plain, out) = run("m.py : f : int\n");
    assert_eq!(plain, Some(1), "{out}");
    let (code, out) = run("m.py : f : int[1..9999]\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("int[1..9999]"), "the PR comment names the domain: {out}");
}

#[test]
fn pr_manifest_accepts_a_value_set_from_the_package() {
    // The charset_normalizer shape: a function inside a package takes one of
    // the names listed in a sibling constants module.
    let d = dir("pr_vs");
    std::fs::create_dir_all(d.join("pkg")).unwrap();
    std::fs::write(d.join("pkg/__init__.py"), "").unwrap();
    std::fs::write(d.join("pkg/constant.py"), "NAMES = ['utf_8', 'cp1252', 'latin_1']\n").unwrap();
    std::fs::write(d.join("pkg/m.py"), "from .constant import NAMES\n\ndef f(s):\n    return {'utf_8': 1, 'cp1252': 2, 'latin_1': 3}.get(s, 0)\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("pkg/m.py"), "from .constant import NAMES\n\ndef f(s):\n    return {'utf_8': 1, 'cp1252': 2, 'latin_1': 3}[s]\n").unwrap();
    let run = |manifest: &str| {
        std::fs::write(d.join(".equiv-review"), manifest).unwrap();
        let out = Command::new(BIN)
            .args(["review-pr", ".equiv-review", "--base", "HEAD"])
            .current_dir(&d)
            .env("EQUIV_METRICS", "off")
            .output()
            .unwrap();
        (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let (plain, out) = run("pkg/m.py : f : str\n");
    assert_eq!(plain, Some(1), "{out}");
    let (code, out) = run("pkg/m.py : f : str[in .constant.NAMES]\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("str[in .constant.NAMES] (3 values)"), "{out}");
    let (bad, out) = run("pkg/m.py : f : str[in .constant.MISSING]\n");
    assert_eq!(bad, Some(2), "an unresolvable list is could-not-check, never green: {out}");
}
