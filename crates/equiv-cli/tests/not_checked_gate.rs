//! Backlog 0.3c: the PR gate fails only on a divergence by default. "Not
//! checked" (safety stop, refused, error, or a changed function auto-detect
//! could not check) is reported prominently, never as green, and blocks the
//! merge only when the Action input `fail-on-not-checked` is "true". A
//! whole-run error (exit 3: bad manifest, no git repo) always fails the gate:
//! "the check did not run" is not "a function was not checked".
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const ENTRYPOINT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/equiv-review-entrypoint.sh");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_ncg_{name}_{}", std::process::id()));
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

/// `review-pr --auto` where the only changed function has no type hints:
/// nothing diverged, nothing was checked -> exit 2 and a headline that says so.
#[test]
fn auto_with_only_unchecked_functions_exits_2_not_green() {
    let d = dir("auto_unchecked");
    std::fs::write(d.join("m.py"), "def g(x):\n    return x\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def g(x):\n    return x + 0\n").unwrap();
    let out = Command::new(BIN)
        .args(["review-pr", "--auto", "--base", "HEAD"])
        .current_dir(&d)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let headline = stdout.lines().skip_while(|l| !l.starts_with("## equiv review")).skip(1).find(|l| !l.trim().is_empty()).unwrap();
    assert!(headline.contains("Nothing was checked"), "{headline}");
    assert!(!headline.to_lowercase().contains("preserved"), "{headline}");
    assert!(stdout.contains("`g`"), "{stdout}");
}

/// A fake `equiv` that prints a marker comment and exits with the given code,
/// so the entrypoint's gate policy can be tested without a real review.
fn fake_bin(d: &Path, code: u8) -> PathBuf {
    let p = d.join(format!("fake-equiv-{code}"));
    std::fs::write(&p, format!("#!/bin/sh\necho '<!-- equiv-review -->'\necho 'fake comment'\nexit {code}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn gate(d: &Path, code: u8, fail: &str, fail_not_checked: Option<&str>) -> (i32, String) {
    let bin = fake_bin(d, code);
    let mut c = Command::new("bash");
    c.arg(ENTRYPOINT)
        .current_dir(d)
        .env("EQUIV_BIN", &bin)
        .env("EQUIV_AUTO", "true")
        .env("EQUIV_FAIL", fail)
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("GH_TOKEN")
        .env_remove("EQUIV_FAIL_NOT_CHECKED");
    if let Some(v) = fail_not_checked {
        c.env("EQUIV_FAIL_NOT_CHECKED", v);
    }
    let out = c.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn gate_policy_divergence_fails_not_checked_passes_by_default() {
    let d = dir("gate");
    // not checked (2): passes by default, and says so visibly
    let (code, text) = gate(&d, 2, "true", None);
    assert_eq!(code, 0, "not checked must not block the merge by default:\n{text}");
    assert!(text.contains("fake comment"), "the comment is still printed:\n{text}");
    assert!(text.contains("not checked"), "swallowing exit 2 must leave a visible line:\n{text}");
    // not checked (2) with the opt-in: fails
    let (code, text) = gate(&d, 2, "true", Some("true"));
    assert_eq!(code, 2, "fail-on-not-checked=true must fail the gate:\n{text}");
    // divergence (1): fails with the default input, passes with fail-on-divergence=false
    let (code, text) = gate(&d, 1, "true", None);
    assert_eq!(code, 1, "a divergence must fail the gate:\n{text}");
    let (code, text) = gate(&d, 1, "false", None);
    assert_eq!(code, 0, "fail-on-divergence=false still posts and passes:\n{text}");
    assert!(text.contains("fake comment"), "{text}");
    // equivalent (0): passes
    let (code, _) = gate(&d, 0, "true", None);
    assert_eq!(code, 0);
    // whole-run error (3): fails whatever the inputs say
    let (code, text) = gate(&d, 3, "false", Some("false"));
    assert_eq!(code, 3, "a run that did not happen is never green:\n{text}");
}
