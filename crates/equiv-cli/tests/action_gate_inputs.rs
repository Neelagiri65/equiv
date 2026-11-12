//! The Action's wrapper must fail closed. A setting typed as "True" still
//! gates; a value that is neither true nor false stops the job; a base ref
//! that cannot be read fails whatever the fail-* inputs say; deleting the
//! manifest in the PR does not turn a blocking review into a notice; and a
//! comment that cannot be posted (fork PRs) is a warning, with the report in
//! the job log and summary and the job result following the review alone.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const ENTRYPOINT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/equiv-review-entrypoint.sh");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_gin_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn exe(p: &Path, body: &str) {
    std::fs::write(p, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A fake `equiv` that prints a marker comment and exits with `code`.
fn fake_bin(d: &Path, code: u8) -> PathBuf {
    let p = d.join(format!("fake-equiv-{code}"));
    exe(&p, &format!("echo '<!-- equiv-review -->'\necho 'fake comment'\nexit {code}"));
    p
}

fn entry(d: &Path, bin: &Path, env: &[(&str, &str)]) -> (i32, String) {
    let mut c = Command::new("bash");
    c.arg(ENTRYPOINT).current_dir(d).env("EQUIV_BIN", bin).env("EQUIV_METRICS", "off");
    c.env("GIT_CEILING_DIRECTORIES", std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()));
    for k in [
        "EQUIV_AUTO", "EQUIV_FAIL", "EQUIV_FAIL_NOT_CHECKED", "EQUIV_ONLY_LABEL", "EQUIV_KEYLESS", "EQUIV_BASE_INPUT",
        "GH_TOKEN", "GITHUB_EVENT_PATH", "GITHUB_STEP_SUMMARY", "GITHUB_BASE_REF", "EQUIV_SIGNING_KEY",
    ] {
        c.env_remove(k);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
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

/// Base: f listed in the manifest. Working tree: f differs at x == 7.
fn repo(name: &str) -> PathBuf {
    let d = dir(name);
    std::fs::write(d.join("m.py"), "def f(x):\n    return x * 2\n").unwrap();
    std::fs::write(d.join(".equiv-review"), "m.py : f : int\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def f(x):\n    return x * 2 + (1 if x == 7 else 0)\n").unwrap();
    d
}

#[test]
fn true_and_false_gate_in_any_letter_case() {
    let d = dir("case");
    let (code, text) = entry(&d, &fake_bin(&d, 1), &[("EQUIV_AUTO", "true"), ("EQUIV_FAIL", "True")]);
    assert_eq!(code, 1, "fail-on-divergence: True must block:\n{text}");
    let (code, text) = entry(&d, &fake_bin(&d, 2), &[("EQUIV_AUTO", "TRUE"), ("EQUIV_FAIL_NOT_CHECKED", "TRUE")]);
    assert_eq!(code, 2, "fail-on-not-checked: TRUE must block:\n{text}");
    let (code, text) = entry(&d, &fake_bin(&d, 1), &[("EQUIV_AUTO", "true"), ("EQUIV_FAIL", "False")]);
    assert_eq!(code, 0, "False is false:\n{text}");
    // auto: False with no manifest is the documented skip, not auto-detect.
    let (code, text) = entry(&d, &fake_bin(&d, 1), &[("EQUIV_AUTO", "False")]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("skipping") && !text.contains("fake comment"), "{text}");
}

#[test]
fn a_value_that_is_neither_true_nor_false_stops_the_job() {
    let d = dir("junk");
    for (k, v) in [("EQUIV_FAIL", "yes"), ("EQUIV_FAIL_NOT_CHECKED", "1"), ("EQUIV_AUTO", "on"), ("EQUIV_KEYLESS", "ture")] {
        let (code, text) = entry(&d, &fake_bin(&d, 0), &[("EQUIV_AUTO", "true"), (k, v)]);
        assert_ne!(code, 0, "{k}={v} must fail closed:\n{text}");
        assert!(text.contains("::error::") && text.contains(v), "{text}");
        assert!(!text.contains("fake comment"), "the review must not run:\n{text}");
    }
}

#[test]
fn an_unreadable_base_ref_fails_the_job_whatever_the_inputs_say() {
    let d = repo("badbase");
    let (code, text) = entry(
        &d,
        Path::new(BIN),
        &[("EQUIV_BASE_INPUT", "origin/typo"), ("EQUIV_FAIL", "false"), ("EQUIV_FAIL_NOT_CHECKED", "false")],
    );
    assert_eq!(code, 4, "{text}");
    assert!(text.contains("could not be read"), "{text}");
    assert!(text.contains("could not fetch the base branch"), "the failed fetch is shown, not hidden:\n{text}");
    assert!(!text.contains("could not be checked"), "not a per-function result:\n{text}");
}

#[test]
fn deleting_the_manifest_in_the_pr_keeps_the_review_blocking() {
    let d = repo("deleted");
    std::fs::remove_file(d.join(".equiv-review")).unwrap();
    let (code, text) = entry(&d, Path::new(BIN), &[("EQUIV_BASE_INPUT", "HEAD")]);
    assert_eq!(code, 1, "still a manifest review, still blocking:\n{text}");
    assert!(text.contains("DIVERGES") && text.contains("was deleted in this PR"), "{text}");
    assert!(!text.contains("not blocking"), "{text}");
    // With auto: "false" as well: the base manifest still counts as present.
    let (code, text) = entry(&d, Path::new(BIN), &[("EQUIV_BASE_INPUT", "HEAD"), ("EQUIV_AUTO", "false")]);
    assert_eq!(code, 1, "{text}");
}

/// PR context with a `gh` stand-in. `gh_ok` false: every call fails, like a
/// read-only token. Returns (exit code, output, what gh was asked to post).
fn with_pr(name: &str, code: u8, gh_ok: bool, silent_bin: bool) -> (i32, String, String, String) {
    let d = dir(name);
    let bin_dir = d.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let log = d.join("gh.log");
    exe(
        &bin_dir.join("gh"),
        &format!("echo \"$@\" >> \"{}\"\n{}", log.display(), if gh_ok { "exit 0" } else { "echo 'HTTP 403: Resource not accessible by integration' >&2; exit 1" }),
    );
    let event = d.join("event.json");
    std::fs::write(&event, "{\"pull_request\":{\"number\":7,\"labels\":[]}}").unwrap();
    let summary = d.join("summary.md");
    let bin = if silent_bin {
        let p = d.join("silent-equiv");
        exe(&p, &format!("echo 'error: something broke' >&2\nexit {code}"));
        p
    } else {
        fake_bin(&d, code)
    };
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());
    let (rc, text) = entry(
        &d,
        &bin,
        &[
            ("EQUIV_AUTO", "true"),
            ("EQUIV_FAIL", "true"),
            ("PATH", &path),
            ("GH_TOKEN", "x"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
            ("GITHUB_STEP_SUMMARY", summary.to_str().unwrap()),
        ],
    );
    (rc, text, std::fs::read_to_string(&summary).unwrap_or_default(), std::fs::read_to_string(&log).unwrap_or_default())
}

#[test]
fn a_comment_that_cannot_be_posted_is_a_warning_and_the_result_follows_the_review() {
    let (code, text, summary, gh) = with_pr("fork_ok", 0, false, false);
    assert!(gh.contains("issues/7/comments"), "the post was attempted: {gh}");
    assert_eq!(code, 0, "a clean review on a fork PR must not go red:\n{text}");
    assert!(text.contains("::warning::") && text.contains("could not post"), "{text}");
    assert!(text.contains("fake comment"), "the report is in the job log:\n{text}");
    assert!(summary.contains("fake comment"), "and in the job summary: {summary}");

    let (code, text, summary, _) = with_pr("fork_div", 1, false, false);
    assert_eq!(code, 1, "a divergence still blocks when the comment cannot be posted:\n{text}");
    assert!(summary.contains("fake comment"), "{summary}");
}

#[test]
fn a_run_that_did_not_happen_says_so_in_the_comment_instead_of_leaving_the_old_one() {
    let (code, text, summary, gh) = with_pr("norun", 3, true, true);
    assert_eq!(code, 3, "{text}");
    assert!(gh.contains("did not run"), "the sticky comment is replaced: {gh}");
    assert!(summary.contains("did not run"), "{summary}");
}

#[test]
fn labels_that_cannot_be_read_are_not_treated_as_no_label() {
    let d = dir("badevent");
    let event = d.join("event.json");
    std::fs::write(&event, "{ this is not json").unwrap();
    let (code, text) = entry(
        &d,
        &fake_bin(&d, 0),
        &[("EQUIV_AUTO", "true"), ("EQUIV_ONLY_LABEL", "refactor"), ("GITHUB_EVENT_PATH", event.to_str().unwrap())],
    );
    assert_ne!(code, 0, "{text}");
    assert!(!text.contains("skipping"), "{text}");
}

#[test]
fn keyless_without_cosign_is_said_out_loud() {
    let d = dir("keyless");
    // A PATH with bash and coreutils but (almost certainly) no cosign.
    let (code, text) = entry(&d, &fake_bin(&d, 0), &[("EQUIV_AUTO", "true"), ("EQUIV_KEYLESS", "True"), ("PATH", "/usr/bin:/bin")]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("::warning::") && text.contains("cosign"), "{text}");
}
