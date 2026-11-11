//! Command-line arguments are read strictly: a value equiv cannot use is a
//! usage error (exit 3) that names it, never something silently dropped. The
//! verdict exit codes (0/1/2) are unchanged.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_args_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("c.py"), "def f(x):\n    return x + x\n").unwrap();
    std::fs::write(d.join("r.py"), "def f(x):\n    return x * 2\n").unwrap();
    d
}

fn run(d: &Path, args: &[&str], key: Option<&str>) -> (Option<i32>, String, String) {
    let mut c = Command::new(BIN);
    c.args(args).current_dir(d).env("EQUIV_METRICS", "off").env_remove("EQUIV_SIGNING_KEY");
    if let Some(k) = key {
        c.env("EQUIV_SIGNING_KEY", k);
    }
    let out = c.output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn non_ascii_hex_is_a_clean_error_not_a_panic() {
    let d = dir("hex");
    let (code, _, err) = run(&d, &["verify-receipt", "aé1"], None);
    assert_eq!(code, Some(3), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.contains("hex"), "{err}");
}

#[test]
fn a_non_numeric_case_count_or_seed_is_a_usage_error() {
    let d = dir("n");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "abc", "--seed", "9"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(!out.contains("EQUIVALENT"), "nothing may run on a bad command line: {out}");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "abc"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("abc"), "the bad value is named: {err}");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "20", "x9"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("x9"), "{err}");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "20", "9", "extra"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("extra"), "{err}");
    // The numeric forms keep working.
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "20", "9"], None);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("seed 9"), "{out}");
}

#[test]
fn an_unknown_flag_is_a_usage_error_naming_the_flag() {
    let d = dir("flag");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "--no-harvest"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("--no-harvest"), "{err}");
    assert!(!out.contains("EQUIVALENT"), "{out}");
    let (code, out, err) = run(&d, &["review-pr", "--auto", "--bse", "HEAD"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("--bse"), "{err}");
    let (code, _, err) = run(&d, &["review-pr", "--auto", "--base"], None);
    assert_eq!(code, Some(3), "{err}");
    assert!(err.contains("--base"), "{err}");
    // Every documented flag of `review` is still accepted.
    let (code, out, err) = run(
        &d,
        &["review", "c.py", "r.py", "f", "int", "--all", "--errors-agree", "--same-host", "--budget", "1000000", "--inputs-from", "c.py"],
        None,
    );
    assert_eq!(code, Some(0), "{out}{err}");
}

#[test]
fn sign_without_a_usable_key_fails_before_anything_runs() {
    let d = dir("sign");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "--sign"], None);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(out.is_empty(), "no verdict may be printed when the run is going to fail: {out}");
    assert!(err.contains("EQUIV_SIGNING_KEY"), "{err}");
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "--sign"], Some("nothex"));
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    // 64 bytes, but not ASCII: must be an error, not a panic.
    let odd = format!("{}é", "a".repeat(62));
    let (code, out, err) = run(&d, &["review", "c.py", "r.py", "f", "int", "--sign"], Some(&odd));
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn explicit_help_exits_zero_and_no_arguments_stays_a_usage_error() {
    let d = dir("help");
    for h in ["--help", "-h"] {
        let (code, out, err) = run(&d, &[h], None);
        assert_eq!(code, Some(0), "{h}: {err}");
        assert!(out.contains("USAGE"), "help goes to stdout: {out}");
    }
    let (code, out, _) = run(&d, &["review", "--help"], None);
    assert_eq!(code, Some(0));
    assert!(out.contains("USAGE"), "{out}");
    let (code, _, err) = run(&d, &[], None);
    assert_eq!(code, Some(3));
    assert!(err.contains("USAGE"), "{err}");
    let (code, _, _) = run(&d, &["no-such-command"], None);
    assert_eq!(code, Some(3));
}

#[test]
fn a_manifest_line_with_a_non_numeric_count_or_seed_is_an_error() {
    let d = dir("manifest");
    for args in [&["init", "-q"][..], &["add", "."], &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "base"]] {
        assert!(Command::new("git").args(args).current_dir(&d).output().unwrap().status.success());
    }
    for bad in ["c.py : f : int : abc\n", "c.py : f : int : 20 : x\n", "c.py : f : int : 20 : 1 : more\n"] {
        std::fs::write(d.join(".equiv-review"), bad).unwrap();
        let (code, out, err) = run(&d, &["review-pr", ".equiv-review", "--base", "HEAD"], None);
        assert_eq!(code, Some(3), "{bad}: {out}{err}");
        assert!(err.contains(".equiv-review:1"), "the line is named: {err}");
    }
}
