//! Constraint: equiv writes NO metrics file unless the user turns metrics on.
//! A trust tool must not leave files in someone's repo that they did not ask
//! for (decision 2026-11-04, backlog 1.3). `DO_NOT_TRACK` overrides an opt-in.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn repo(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_telemetry_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x * 2\n").unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "base"]);
    std::fs::write(d.join("m.py"), "def f(x: int):\n    return x + x\n").unwrap();
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

/// Runs `review-pr --auto` with a clean metrics environment plus `env`.
fn run(d: &Path, env: &[(&str, &str)]) {
    let mut c = Command::new(BIN);
    c.args(["review-pr", "--auto", "--base", "HEAD"])
        .current_dir(d)
        .env_remove("EQUIV_METRICS")
        .env_remove("EQUIV_METRICS_OUT")
        .env_remove("DO_NOT_TRACK")
        .env("GIT_CEILING_DIRECTORIES", std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()));
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("## equiv review"), "review-pr did not run: {out:?}");
}

#[test]
fn no_metrics_file_by_default() {
    let d = repo("default");
    run(&d, &[]);
    assert!(!d.join("equiv-metrics.jsonl").exists(), "metrics written without opt-in");
}

#[test]
fn metrics_written_only_when_turned_on() {
    let d = repo("on");
    run(&d, &[("EQUIV_METRICS", "on")]);
    let body = std::fs::read_to_string(d.join("equiv-metrics.jsonl")).expect("opt-in must write the file");
    assert_eq!(body.lines().count(), 1);
}

#[test]
fn do_not_track_overrides_opt_in() {
    let d = repo("dnt");
    run(&d, &[("EQUIV_METRICS", "on"), ("DO_NOT_TRACK", "1")]);
    assert!(!d.join("equiv-metrics.jsonl").exists(), "DO_NOT_TRACK ignored");
}

#[test]
fn any_other_value_stays_off() {
    let d = repo("other");
    run(&d, &[("EQUIV_METRICS", "yes please")]);
    assert!(!d.join("equiv-metrics.jsonl").exists(), "only EQUIV_METRICS=on opts in");
}
