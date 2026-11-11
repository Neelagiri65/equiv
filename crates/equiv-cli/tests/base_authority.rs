//! The manifest at the base ref decides what `review-pr` checks. The PR under
//! review can add lines, but it cannot switch a check off by emptying,
//! deleting or narrowing the manifest it ships with: lines listed at base run
//! as written at base, and the output says the two manifests differ.
//! Also here: an unreadable base ref is its own exit code (4), a listed
//! function that is gone counts as a behaviour change, and the base files are
//! kept in a private directory that is removed afterwards.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const BASE_PY: &str = "def f(x):\n    return x * 2\n\ndef g(x):\n    return x + 1\n";
/// f now differs from base at x == 7.
const CHANGED_PY: &str = "def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n\ndef g(x):\n    return x + 1\n";

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

/// A committed base holding `files`; the working tree is then the "PR".
fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_base_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (f, body) in files {
        std::fs::write(d.join(f), body).unwrap();
    }
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    d
}

fn review_pr(d: &Path, extra: &[&str]) -> (Option<i32>, String, String) {
    let tmp = d.join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    let out = Command::new(BIN)
        .args(["review-pr", ".equiv-review", "--base", "HEAD"])
        .args(extra)
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        .env("TMPDIR", &tmp)
        .env_remove("EQUIV_SIGNING_KEY")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn replacing_the_manifest_with_a_comment_does_not_switch_the_check_off() {
    let d = repo("comment", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("m.py"), CHANGED_PY).unwrap();
    std::fs::write(d.join(".equiv-review"), "# nothing to see\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("DIVERGES"), "{out}");
    assert!(out.contains("differs from base"), "{out}");
    assert!(out.contains("run as written at base"), "{out}");
    assert!(out.contains("`m.py : f : int`"), "the base line is named: {out}");
}

#[test]
fn deleting_the_manifest_does_not_switch_the_check_off() {
    let d = repo("deleted", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("m.py"), CHANGED_PY).unwrap();
    std::fs::remove_file(d.join(".equiv-review")).unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("DIVERGES"), "{out}");
    assert!(out.contains("was deleted in this PR"), "{out}");
}

#[test]
fn narrowing_a_line_runs_the_base_line_as_well_as_the_new_one() {
    let d = repo("narrow", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("m.py"), CHANGED_PY).unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int[5..5]\n").unwrap();
    let (code, out, err) = review_pr(&d, &["--attest-out", "att"]);
    assert_eq!(code, Some(1), "the unrestricted base line still finds x == 7: {out}{err}");
    assert!(out.contains("DIVERGES"), "{out}");
    assert_eq!(out.matches("| `f` |").count(), 2, "both the base line and the edited line ran: {out}");
    assert!(out.contains("New in this PR (also run; the last row"), "{out}");
    assert!(out.contains("`m.py : f : int[5..5]`"), "{out}");
    // One statement per function name: the base line's, not the narrowed one's.
    let stmt = std::fs::read_to_string(d.join("att/f.intoto.json")).unwrap();
    assert!(stmt.contains("counterexample"), "{stmt}");
}

#[test]
fn a_line_added_by_the_pr_runs_too_and_is_named_as_new() {
    let d = repo("added", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("m.py"), "def f(x):\n    return x + x\n\ndef g(x):\n    return x + 2\n").unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int\nm.py : g : int\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "the new line's function diverges and that counts: {out}{err}");
    assert!(out.contains("| `f` | equivalent |"), "{out}");
    assert!(out.contains("| `g` | DIVERGES |"), "{out}");
    assert!(out.contains("New in this PR"), "{out}");
    assert!(out.contains("`m.py : g : int`"), "{out}");
}

#[test]
fn an_unchanged_manifest_adds_no_note_and_whitespace_is_not_a_difference() {
    let d = repo("same", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("m.py"), "def f(x):\n    return x + x\n\ndef g(x):\n    return x + 1\n").unwrap();
    std::fs::write(d.join(".equiv-review"), "# a comment\n  m.py:f:int  \n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(!out.contains("Manifest:"), "{out}");
    assert!(out.starts_with("<!-- equiv-review -->"), "{out}");
}

#[test]
fn a_manifest_that_is_new_in_the_pr_is_used_and_said_to_be_new() {
    let d = repo("new", &[("m.py", BASE_PY)]);
    std::fs::write(d.join("m.py"), CHANGED_PY).unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("is new in this PR"), "{out}");
    assert!(out.starts_with("<!-- equiv-review -->"), "the sticky marker stays first: {out}");
}

#[test]
fn no_manifest_at_base_or_in_the_pr_is_an_error() {
    let d = repo("none", &[("m.py", BASE_PY)]);
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(3), "{out}{err}");
}

#[test]
fn a_twin_line_at_base_cannot_be_dropped_by_the_pr() {
    let d = repo(
        "twin",
        &[
            ("fast.py", "def f(x):\n    return x * 2\n"),
            ("slow.py", "def f(x):\n    return x + x\n"),
            (".equiv-review", "twin : fast.py : slow.py : f : int\n"),
        ],
    );
    std::fs::write(d.join("fast.py"), "def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n").unwrap();
    std::fs::write(d.join(".equiv-review"), "").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("`fast.py` returns"), "{out}");
    assert!(out.contains("differs from base"), "{out}");
}

// ---- the base ref must be readable ----

#[test]
fn a_base_ref_that_does_not_resolve_is_exit_4_not_could_not_check() {
    let d = repo("badref", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\nm.py : g : int\n")]);
    for mode in [&[".equiv-review"][..], &["--auto"]] {
        let out = Command::new(BIN)
            .arg("review-pr")
            .args(mode)
            .args(["--base", "origin/typo"])
            .current_dir(&d)
            .env("EQUIV_METRICS", "off")
            .output()
            .unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.status.code(), Some(4), "{mode:?}: {stdout}{stderr}");
        assert!(stdout.is_empty(), "no comment for a review that did not run: {stdout}");
        assert!(stderr.contains("cannot read the base ref `origin/typo`"), "{stderr}");
    }
}

#[test]
fn a_file_that_is_new_since_base_is_not_a_base_ref_problem() {
    let d = repo("newfile", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::write(d.join("n.py"), "def h(x):\n    return x\n").unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int\nn.py : h : int\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(2), "nothing to compare h with: could not check, not exit 4: {out}{err}");
    assert!(out.contains("| `f` | equivalent |"), "{out}");
}

// ---- a listed function that is gone ----

#[test]
fn a_listed_function_that_was_renamed_or_deleted_is_a_behaviour_change() {
    let d = repo("renamed", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\nm.py : g : int\n")]);
    std::fs::write(d.join("m.py"), "def f2(x):\n    return x * 2\n\ndef g(x):\n    return x + 1\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "same status as a divergence: {out}{err}");
    assert!(out.contains("`f` no longer exists in `m.py`; it was removed or renamed"), "{out}");
    assert!(out.contains("| `g` | equivalent |"), "the other line still ran: {out}");
    let first = out.lines().skip_while(|l| !l.starts_with("## equiv review")).skip(1).find(|l| !l.trim().is_empty()).unwrap();
    assert!(first.contains("Behaviour changed"), "the removal leads the comment: {first}");
}

#[test]
fn a_deleted_file_with_the_only_listed_function_never_reads_as_preserved() {
    let d = repo("gonefile", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    std::fs::remove_file(d.join("m.py")).unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("`f` no longer exists in `m.py`"), "{out}");
    assert!(!out.contains("preserved against base"), "{out}");
    assert!(!out.contains("0 checked"), "{out}");
    assert!(out.starts_with("<!-- equiv-review -->"), "{out}");
}

#[test]
fn a_twin_function_removed_from_either_file_is_a_behaviour_change() {
    let d = repo(
        "twingone",
        &[
            ("fast.py", "def f(x):\n    return x * 2\n"),
            ("slow.py", "def f(x):\n    return x + x\n"),
            (".equiv-review", "twin : fast.py : slow.py : f : int\n"),
        ],
    );
    std::fs::write(d.join("slow.py"), "def other(x):\n    return x\n").unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("`f` no longer exists in `slow.py`"), "{out}");
}

#[test]
fn a_function_that_never_existed_at_base_stays_could_not_check() {
    let d = repo("never", &[("m.py", BASE_PY), (".equiv-review", "m.py : nope : int\n")]);
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(!out.contains("no longer exists"), "{out}");
}

// ---- where the base files live ----

#[test]
fn base_files_live_in_a_private_directory_that_is_removed() {
    let d = repo("private", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    // The candidate records every equiv_base_* file it can see in the temp
    // directory at import time (the old, guessable location), and tries to
    // replace it with its own code.
    std::fs::write(
        d.join("m.py"),
        "import glob, os, tempfile\n\
         for p in glob.glob(os.path.join(tempfile.gettempdir(), 'equiv_base_*')):\n\
         \x20   open(p, 'w').write(open(__file__).read())\n\
         \x20   open(os.path.join(os.path.dirname(__file__), 'seen.txt'), 'a').write(p + '\\n')\n\
         def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n",
    )
    .unwrap();
    let (code, out, err) = review_pr(&d, &[]);
    assert_eq!(code, Some(1), "the reference was not replaced, so the change is still seen: {out}{err}");
    assert!(!d.join("seen.txt").exists(), "no base file at a guessable path");
    // The evaluator's own scratch files (harvest lists, the driver script)
    // are written by equiv-review and are not part of this check.
    let left: Vec<String> = std::fs::read_dir(d.join("tmp"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("equiv-") || n.starts_with("equiv_base_"))
        .collect();
    assert!(left.is_empty(), "the base files and their directory are removed: {left:?}");
}

#[test]
fn a_manifest_path_outside_the_tree_is_an_error_not_a_new_manifest() {
    let d = repo("abs", &[("m.py", BASE_PY), (".equiv-review", "m.py : f : int\n")]);
    let abs = d.join(".equiv-review");
    let out = Command::new(BIN)
        .args(["review-pr", abs.to_str().unwrap(), "--base", "HEAD"])
        .current_dir(&d)
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn run_from_a_subdirectory_the_base_and_head_copies_are_the_same_files() {
    let d = repo("subdir", &[("m.py", BASE_PY)]);
    std::fs::create_dir_all(d.join("pkg")).unwrap();
    std::fs::write(d.join("pkg/n.py"), "def f(x):\n    return x * 2\n").unwrap();
    std::fs::write(d.join("pkg/.equiv-review"), "n.py : f : int\n").unwrap();
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "pkg"]);
    std::fs::write(d.join("pkg/n.py"), "def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n").unwrap();
    std::fs::write(d.join("pkg/.equiv-review"), "").unwrap();
    let (code, out, err) = review_pr(&d.join("pkg"), &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("DIVERGES") && out.contains("differs from base"), "{out}");
}
