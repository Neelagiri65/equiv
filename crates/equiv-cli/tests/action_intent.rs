//! The gate blocks a merge on a divergence only when the PR has declared that
//! behaviour must not change: the functions are listed in the manifest, or the
//! PR carries the `only-label` label. Plain auto-detect mode reports a
//! divergence in the comment without blocking, because in an ordinary PR most
//! changed functions change on purpose. An explicit `fail-on-divergence` wins.
use std::path::{Path, PathBuf};
use std::process::Command;

const ENTRYPOINT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/equiv-review-entrypoint.sh");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_intent_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake `equiv` that prints a marker comment and exits with `code`.
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

/// A pull_request event file carrying `labels`.
fn event(d: &Path, labels: &[&str]) -> PathBuf {
    let p = d.join(format!("event-{}.json", labels.join("_")));
    let ls: Vec<String> = labels.iter().map(|l| format!("{{\"name\":\"{l}\"}}")).collect();
    std::fs::write(&p, format!("{{\"pull_request\":{{\"labels\":[{}]}}}}", ls.join(","))).unwrap();
    p
}

struct Run<'a> {
    auto: &'a str,
    fail: Option<&'a str>,
    only_label: Option<&'a str>,
    labels: Option<&'a [&'a str]>,
    manifest: bool,
}

fn run(name: &str, code: u8, r: Run) -> (i32, String) {
    let d = dir(name);
    if r.manifest {
        std::fs::write(d.join(".equiv-review"), "m.py : f : int\n").unwrap();
    }
    let bin = fake_bin(&d, code);
    let mut c = Command::new("bash");
    c.arg(ENTRYPOINT)
        .current_dir(&d)
        .env("EQUIV_BIN", &bin)
        .env("EQUIV_AUTO", r.auto)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("EQUIV_FAIL")
        .env_remove("EQUIV_ONLY_LABEL")
        .env_remove("EQUIV_FAIL_NOT_CHECKED");
    if let Some(f) = r.fail {
        c.env("EQUIV_FAIL", f);
    }
    if let Some(l) = r.only_label {
        c.env("EQUIV_ONLY_LABEL", l);
    }
    if let Some(ls) = r.labels {
        c.env("GITHUB_EVENT_PATH", event(&d, ls));
    }
    let out = c.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn auto_mode_reports_a_divergence_without_blocking_by_default() {
    let (code, text) = run("auto", 1, Run { auto: "true", fail: None, only_label: None, labels: None, manifest: false });
    assert_eq!(code, 0, "auto-detect without declared intent must not block:\n{text}");
    assert!(text.contains("fake comment"), "the comment is still posted:\n{text}");
    assert!(text.contains("not blocking"), "the notice says it did not block:\n{text}");
}

#[test]
fn manifest_mode_blocks_a_divergence_by_default() {
    let (code, text) = run("manifest", 1, Run { auto: "false", fail: None, only_label: None, labels: None, manifest: true });
    assert_eq!(code, 1, "listed functions declare intent; a divergence must block:\n{text}");
}

#[test]
fn explicit_fail_on_divergence_wins_in_both_modes() {
    let (code, text) = run("auto_true", 1, Run { auto: "true", fail: Some("true"), only_label: None, labels: None, manifest: false });
    assert_eq!(code, 1, "explicit true blocks in auto mode:\n{text}");
    let (code, text) = run("man_false", 1, Run { auto: "false", fail: Some("false"), only_label: None, labels: None, manifest: true });
    assert_eq!(code, 0, "explicit false does not block in manifest mode:\n{text}");
}

#[test]
fn only_label_skips_unlabelled_prs_and_blocks_labelled_ones() {
    let (code, text) = run(
        "unlabelled",
        1,
        Run { auto: "true", fail: None, only_label: Some("refactor"), labels: Some(&["feature"]), manifest: false },
    );
    assert_eq!(code, 0, "an unlabelled PR is skipped:\n{text}");
    assert!(text.contains("skipping"), "{text}");
    assert!(!text.contains("fake comment"), "the review must not run on an unlabelled PR:\n{text}");

    let (code, text) = run(
        "labelled",
        1,
        Run { auto: "true", fail: None, only_label: Some("refactor"), labels: Some(&["refactor"]), manifest: false },
    );
    assert_eq!(code, 1, "a labelled PR declares intent; a divergence must block:\n{text}");

    let (code, text) = run(
        "noevent",
        1,
        Run { auto: "true", fail: None, only_label: Some("refactor"), labels: None, manifest: false },
    );
    assert_eq!(code, 0, "no event file means no label, so skip:\n{text}");
    assert!(text.contains("skipping"), "{text}");
}

#[test]
fn a_run_that_did_not_happen_still_fails_whatever_the_intent() {
    let (code, text) = run("err", 3, Run { auto: "true", fail: None, only_label: None, labels: None, manifest: false });
    assert_eq!(code, 3, "exit 3 is never green:\n{text}");
}
