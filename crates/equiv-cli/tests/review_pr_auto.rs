//! `review-pr --auto` end to end against a real git repo: a changed, hinted
//! function is found from the diff and checked; a head file that no longer
//! parses is a whole-run error, never a green "0 checked functions". (The
//! mutation audit of 2026-10-30 found both paths unasserted.)
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_auto_{name}_{}", std::process::id()));
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

fn run_auto(d: &Path) -> (Option<i32>, String, String) {
    let out = Command::new(BIN)
        .args(["review-pr", "--auto", "--base", "HEAD"])
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn headline(stdout: &str) -> &str {
    stdout.lines().skip_while(|l| !l.starts_with("## equiv review")).skip(1).find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// The diff names one changed file; its changed, hinted function is checked
/// and reported by name. A silently empty diff would read "0 checked".
#[test]
fn auto_checks_the_function_changed_in_the_diff() {
    let d = dir("real_diff");
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n\ndef h(x: int):\n    return x\n").unwrap();
    std::fs::write(d.join("other.py"), "def k(x: int):\n    return x\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x + x\n\ndef h(x: int):\n    return x\n").unwrap();
    let (code, stdout, stderr) = run_auto(&d);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
    let h = headline(&stdout);
    assert_eq!(h, "1 checked function: behaviour preserved against base.", "{stdout}");
    assert!(stdout.contains("| `f` | equivalent |"), "{stdout}");
    assert!(!stdout.contains("`h`") && !stdout.contains("`k`"), "unchanged functions must not appear: {stdout}");

    // A divergence in the diffed function fails the gate with the input.
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2 if x != 777 else 0\n\ndef h(x: int):\n    return x\n").unwrap();
    let (code, stdout, stderr) = run_auto(&d);
    assert_eq!(code, Some(1), "{stdout}\n{stderr}");
    assert_eq!(headline(&stdout), "1 of 1 checked function changed behaviour.", "{stdout}");
    assert!(stdout.contains("| `f` | DIVERGES |") && stdout.contains("777"), "{stdout}");
}

/// A head file that no longer parses must not pass as "nothing changed".
#[test]
fn auto_with_unparseable_head_is_not_green() {
    let d = dir("broken_head");
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def f(x: int:\n    return x\n").unwrap();
    let (code, stdout, stderr) = run_auto(&d);
    assert_ne!(code, Some(0), "a broken head must not pass the gate: {stdout}");
    assert!(!stdout.contains("preserved"), "{stdout}");
    assert!(!stdout.contains("checked function"), "no checked-count headline on a broken head: {stdout}");
    let all = format!("{stdout}\n{stderr}");
    assert!(all.contains("could not be parsed"), "{all}");
}

/// Outside a git repo (or with a base ref that does not exist) `--auto` is a
/// whole-run error, not an empty green review.
#[test]
fn auto_without_a_usable_git_base_is_an_error() {
    let d = dir("no_git");
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x\n").unwrap();
    let (code, stdout, stderr) = run_auto(&d);
    assert_eq!(code, Some(3), "{stdout}\n{stderr}");
    assert!(!stdout.contains("preserved"), "{stdout}");
    assert!(stderr.contains("git diff failed"), "{stderr}");
}
