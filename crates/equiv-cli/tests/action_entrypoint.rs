//! The Action's wrapper script, run locally against the built binary. CI used
//! to exercise only the released action (the repo workflow pinned a tag), so a
//! wrapper change shipped untested. No PR context is set, so nothing is posted.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_entry_{name}_{}", std::process::id()));
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

/// A repo where both f and g changed; only f is listed when a manifest exists.
fn repo(name: &str, manifest: bool) -> PathBuf {
    let d = dir(name);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n\ndef g(x: int):\n    return x\n").unwrap();
    if manifest {
        std::fs::write(d.join(".equiv-review"), "m.py : f : int\n").unwrap();
    }
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x + x\n\ndef g(x: int):\n    return x + 0\n").unwrap();
    d
}

fn entry(d: &Path, auto: Option<&str>) -> (Option<i32>, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/equiv-review-entrypoint.sh");
    let mut c = Command::new("bash");
    c.arg(script)
        .current_dir(d)
        .env("EQUIV_BIN", BIN)
        .env("EQUIV_BASE_INPUT", "HEAD")
        .env("EQUIV_METRICS", "off")
        .env("GIT_CEILING_DIRECTORIES", std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()))
        .env_remove("EQUIV_AUTO")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("GH_TOKEN")
        .env_remove("EQUIV_SIGNING_KEY");
    if let Some(a) = auto {
        c.env("EQUIV_AUTO", a);
    }
    let out = c.output().unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// Zero config: no manifest and `auto` left unset reviews every changed
/// function found in the diff. It used to print "skipping" and pass.
#[test]
fn no_manifest_and_auto_unset_detects_from_the_diff() {
    let d = repo("nomanifest", false);
    let (code, out) = entry(&d, None);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("| `f` | equivalent |") && out.contains("| `g` | equivalent |"), "{out}");
    assert!(!out.contains("skipping"), "{out}");
}

/// An existing manifest keeps working exactly as before when `auto` is unset.
#[test]
fn manifest_present_and_auto_unset_uses_the_manifest() {
    let d = repo("withmanifest", true);
    let (code, out) = entry(&d, None);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("| `f` | equivalent |"), "{out}");
    assert!(!out.contains("`g`"), "g is not in the manifest:\n{out}");
}

/// An explicit `auto: "false"` with no manifest is the user's choice: skip.
#[test]
fn explicit_auto_false_without_a_manifest_skips() {
    let d = repo("explicitfalse", false);
    let (code, out) = entry(&d, Some("false"));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("skipping"), "{out}");
}

/// An explicit `auto: "true"` ignores a manifest, as documented.
#[test]
fn explicit_auto_true_ignores_the_manifest() {
    let d = repo("explicittrue", true);
    let (code, out) = entry(&d, Some("true"));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("| `g` | equivalent |"), "{out}");
}
