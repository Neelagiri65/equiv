//! Parity in the Action (2026-11-10): `review-pr` compared a function only with
//! its own base-branch version, so the twin checks (a Rust or compiled port vs
//! its Python reference) could only be run by hand. A manifest line
//! `twin : cand.py : ref.py : func : sig [: flags]` checks two files of the
//! PR's own tree against each other on every PR. Written before the change.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn repo(name: &str, files: &[(&str, &str)], manifest: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_twin_{name}_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    for (f, body) in files {
        std::fs::write(d.join(f), body).unwrap();
    }
    std::fs::write(d.join(".equiv-review"), manifest).unwrap();
    for args in [&["init", "-q"][..], &["add", "."], &["commit", "-q", "-m", "base"]] {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&d)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }
    d
}

fn review_pr(d: &Path) -> (Option<i32>, String) {
    let out = Command::new(BIN)
        .args(["review-pr", ".equiv-review", "--base", "HEAD"])
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn twins_that_agree_pass_and_are_named() {
    let d = repo("agree", &[("fast.py", "def f(x):\n    return x * 2\n"), ("slow.py", "def f(x):\n    return x + x\n")], "twin : fast.py : slow.py : f : int\n");
    let (code, out) = review_pr(&d);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("fast.py") && out.contains("slow.py"), "the comment names both files: {out}");
}

#[test]
fn twins_that_differ_block_and_say_which_file_returned_what() {
    let d = repo("differ", &[("fast.py", "def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n"), ("slow.py", "def f(x):\n    return x + x\n")], "twin : fast.py : slow.py : f : int\n");
    let (code, out) = review_pr(&d);
    assert_eq!(code, Some(1), "a twin difference blocks: {out}");
    assert!(out.contains("`fast.py` returns") && out.contains("`slow.py` returns"), "{out}");
    assert!(!out.contains("this PR returns"), "not head-vs-base wording: {out}");
}

#[test]
fn per_line_errors_agree() {
    let files = [
        ("fast.py", "def f(x):\n    if x < 0:\n        raise ValueError('neg')\n    return x * 2\n"),
        ("slow.py", "def f(x):\n    if x < 0:\n        raise TypeError('neg')\n    return x + x\n"),
    ];
    let strict = repo("ea_off", &files, "twin : fast.py : slow.py : f : int\n");
    assert_eq!(review_pr(&strict).0, Some(1));
    let loose = repo("ea_on", &files, "twin : fast.py : slow.py : f : int : errors-agree\n");
    let (code, out) = review_pr(&loose);
    assert_eq!(code, Some(0), "{out}");
}

#[test]
fn per_line_same_host() {
    let files = [("fast.py", "def _h(x):\n    return x * 2.0\n\ndef f(x):\n    return _h(x)\n"), ("slow.py", "def f(x):\n    return x + x\n")];
    let off = repo("sh_off", &files, "twin : fast.py : slow.py : f : float\n");
    assert_eq!(review_pr(&off).0, Some(2), "refused without same-host");
    let on = repo("sh_on", &files, "twin : fast.py : slow.py : f : float : same-host\n");
    let (code, out) = review_pr(&on);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("SAME HOST ONLY"), "{out}");
}

#[test]
fn twin_and_ordinary_lines_mix() {
    let d = repo(
        "mix",
        &[("fast.py", "def f(x):\n    return x * 2\n"), ("slow.py", "def f(x):\n    return x + x\n"), ("m.py", "def g(x):\n    return x\n")],
        "twin : fast.py : slow.py : f : int\nm.py : g : int\n",
    );
    let (code, out) = review_pr(&d);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("`g`") && out.contains("fast.py"), "{out}");
}

#[test]
fn malformed_twin_lines_are_rejected() {
    for (name, line) in [("short", "twin : fast.py : f : int\n"), ("flag", "twin : fast.py : slow.py : f : int : turbo\n")] {
        let d = repo(name, &[("fast.py", "def f(x):\n    return x\n"), ("slow.py", "def f(x):\n    return x\n")], line);
        let (code, out) = review_pr(&d);
        assert!(code != Some(0) && code != Some(1), "{name}: {out}");
        assert!(out.contains("twin"), "{name}: the error names the twin syntax: {out}");
    }
}
