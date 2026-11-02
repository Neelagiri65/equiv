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
    run_auto_base(d, "HEAD")
}

fn run_auto_base(d: &Path, base: &str) -> (Option<i32>, String, String) {
    let out = Command::new(BIN)
        .args(["review-pr", "--auto", "--base", base])
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        // The temp dir may itself sit inside some git checkout (a CI workspace,
        // a mutation-testing tree): stop git's discovery at the temp root so
        // only the repo each test creates is ever seen.
        .env("GIT_CEILING_DIRECTORIES", std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()))
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

/// A renamed file is still the same functions: the base version is read from
/// the old path and the changed function is checked, not listed as "new".
/// (Backlog 1.2, gap 3: before this, a rename read as delete plus new file
/// and nothing in it was checked.)
#[test]
fn auto_follows_a_renamed_file_and_checks_its_changed_function() {
    let d = dir("rename");
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n\ndef h(x: int):\n    return x\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    git(&d, &["mv", "m.py", "n.py"]);
    std::fs::write(d.join("n.py"), "def f(x: int):\n    return x + x + (1 if x == 777 else 0)\n\ndef h(x: int):\n    return x\n").unwrap();
    let (code, stdout, stderr) = run_auto(&d);
    assert_eq!(code, Some(1), "f diverges at 777 and must be checked by name:\n{stdout}\n{stderr}");
    assert!(stdout.contains("| `f` | DIVERGES |"), "{stdout}");
    assert!(!stdout.contains("new function"), "a rename is not a new file:\n{stdout}");
    assert!(!stdout.contains("removed in this change"), "{stdout}");
}

/// A local run from a branch that is behind its base: only the branch's own
/// changes are reviewed. A function changed on the base branch after the
/// branch point is not "changed in this PR" (and must not be reported with
/// the roles reversed). (Backlog 1.2, gap 2: diff against the merge-base.)
#[test]
fn auto_diffs_against_the_merge_base_not_the_base_tip() {
    let d = dir("mergebase");
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n\ndef g(x: int):\n    return x\n").unwrap();
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    git(&d, &["checkout", "-q", "-b", "pr"]);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x + x\n\ndef g(x: int):\n    return x\n").unwrap();
    git(&d, &["commit", "-q", "-am", "pr: f rewritten"]);
    git(&d, &["checkout", "-q", "main"]);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n\ndef g(x: int):\n    return x + 1\n").unwrap();
    git(&d, &["commit", "-q", "-am", "main moved on: g changed"]);
    git(&d, &["checkout", "-q", "pr"]);
    let (code, stdout, stderr) = run_auto_base(&d, "main");
    assert_eq!(code, Some(0), "only f changed on the branch, and it is equivalent:\n{stdout}\n{stderr}");
    assert!(stdout.contains("| `f` | equivalent |"), "{stdout}");
    assert!(!stdout.contains("`g`"), "g changed on main, not in this PR:\n{stdout}");
}

/// Concurrent reviews on one machine must not share the temp file that holds
/// a function's base version. It used to be named after the function and
/// file only, so two runs reviewing `f` in `m.py` overwrote each other's base
/// and one could judge against the other's code (main went red on an ARM
/// runner this way, 2026-11-02). Each run here has a different base; every
/// verdict must match its own repo.
#[test]
fn concurrent_reviews_do_not_share_a_base_file() {
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                let d = dir(&format!("concurrent{i}"));
                let base = format!("def f(x: int):\n    return x * 2 + {i}\n");
                std::fs::write(d.join("m.py"), &base).unwrap();
                git(&d, &["init", "-q"]);
                git(&d, &["add", "."]);
                git(&d, &["commit", "-q", "-m", "base"]);
                // Even i: an exact rewrite (exit 0). Odd i: diverges (exit 1).
                let head = if i % 2 == 0 {
                    format!("def f(x: int):\n    return x + x + {i}\n")
                } else {
                    format!("def f(x: int):\n    return x + x + {}\n", i + 1)
                };
                std::fs::write(d.join("m.py"), head).unwrap();
                let mut codes = Vec::new();
                for _ in 0..3 {
                    codes.push(run_auto(&d).0);
                }
                (i, codes)
            })
        })
        .collect();
    for h in handles {
        let (i, codes) = h.join().unwrap();
        let want = if i % 2 == 0 { Some(0) } else { Some(1) };
        assert!(codes.iter().all(|c| *c == want), "run {i}: expected {want:?} every time, got {codes:?}");
    }
}
