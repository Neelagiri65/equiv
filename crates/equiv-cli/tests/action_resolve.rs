//! Constraint tests for the Action's binary resolution (written before the fix).
//! The property: a workflow that pins `Neelagiri65/equiv@vX.Y.Z` must run engine
//! X.Y.Z, never "the latest release". Before this fix the Action downloaded the
//! latest release whatever tag was pinned, so pinned results changed under users.
//! The resolver reads the version from the Action's own source (the pinned
//! checkout), downloads exactly that release, and otherwise builds that source.
//! `gh` and `cargo` are replaced by stand-ins that record how they were called.
use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_resolve_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stub(bin: &Path, name: &str, body: &str) {
    let p = bin.join(name);
    std::fs::write(&p, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A pinned Action checkout whose Cargo.toml says `version`.
fn action_checkout(d: &Path, version: Option<&str>) -> PathBuf {
    let a = d.join("action");
    std::fs::create_dir_all(&a).unwrap();
    let toml = match version {
        Some(v) => format!("[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"{v}\"\nedition = \"2021\"\n"),
        None => "[workspace]\nmembers = []\n".to_string(),
    };
    std::fs::write(a.join("Cargo.toml"), toml).unwrap();
    a
}

/// `gh` stand-in: logs its arguments; when `ok`, writes the requested asset
/// (a tar.xz holding an `equiv` file) into --dir, like `gh release download`.
fn fake_gh(bin: &Path, log: &Path, ok: bool) {
    let body = format!(
        r#"echo "$@" >> "{log}"
{tail}"#,
        log = log.display(),
        tail = if ok {
            r#"dir=""; pat=""
while [[ $# -gt 0 ]]; do case "$1" in --dir) dir="$2"; shift 2;; --pattern) pat="$2"; shift 2;; *) shift;; esac; done
mkdir -p "$dir/pkg" && printf '#!/bin/sh\necho fake-equiv\n' > "$dir/pkg/equiv"
tar -cJf "$dir/$pat" -C "$dir" pkg && rm -rf "$dir/pkg""#
        } else {
            "exit 1"
        }
    );
    stub(bin, "gh", &body);
}

/// `cargo` stand-in: logs its arguments and working directory, then produces
/// target/release/equiv like a real build.
fn fake_cargo(bin: &Path, log: &Path) {
    stub(
        bin,
        "cargo",
        &format!(
            r#"echo "$PWD :: $@" >> "{log}"
mkdir -p target/release && printf '#!/bin/sh\necho built-equiv\n' > target/release/equiv && chmod +x target/release/equiv"#,
            log = log.display()
        ),
    );
}

fn run(d: &Path, action: &Path) -> (Option<i32>, String, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/resolve-equiv-binary.sh");
    let env_file = d.join("github_env");
    std::fs::write(&env_file, "").unwrap();
    let bin = d.join("bin");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new("bash")
        .arg(script)
        .env("PATH", path)
        .env("ACTION_PATH", action)
        .env("EQUIV_ACTION_REPO", "Neelagiri65/equiv")
        .env("GITHUB_ENV", &env_file)
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code(), std::fs::read_to_string(&env_file).unwrap(), log)
}

#[test]
fn pinned_version_is_downloaded_never_latest() {
    let d = dir("pinned");
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh_log = d.join("gh.log");
    fake_gh(&bin, &gh_log, true);
    fake_cargo(&bin, &d.join("cargo.log"));
    let action = action_checkout(&d, Some("9.8.7"));
    let (code, env, log) = run(&d, &action);
    assert_eq!(code, Some(0), "{log}");
    let calls = std::fs::read_to_string(&gh_log).unwrap();
    assert!(calls.starts_with("release download v9.8.7 "), "must ask for the pinned tag, got: {calls}");
    assert_eq!(calls.lines().count(), 1, "exactly one download, got: {calls}");
    let line = env.lines().find(|l| l.starts_with("EQUIV_BIN=")).expect("EQUIV_BIN exported");
    let p = PathBuf::from(line.trim_start_matches("EQUIV_BIN="));
    assert!(p.is_file(), "EQUIV_BIN points at the extracted binary: {line}");
    assert!(!d.join("cargo.log").exists(), "no build when the pinned release exists");
}

#[test]
fn missing_release_builds_the_pinned_source_not_latest() {
    let d = dir("build");
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh_log = d.join("gh.log");
    let cargo_log = d.join("cargo.log");
    fake_gh(&bin, &gh_log, false);
    fake_cargo(&bin, &cargo_log);
    let action = action_checkout(&d, Some("9.8.8"));
    let (code, env, log) = run(&d, &action);
    assert_eq!(code, Some(0), "{log}");
    let calls = std::fs::read_to_string(&gh_log).unwrap();
    assert!(calls.lines().all(|l| l.starts_with("release download v9.8.8 ")), "only the pinned tag is tried: {calls}");
    let built = std::fs::read_to_string(&cargo_log).expect("falls back to building the pinned source");
    assert!(built.contains(&format!("{} ::", action.display())), "builds inside the Action checkout: {built}");
    assert!(env.contains(&format!("EQUIV_BIN={}", action.join("target/release/equiv").display())), "{env}");
}

#[test]
fn unreadable_version_fails_loudly() {
    let d = dir("noversion");
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh_log = d.join("gh.log");
    fake_gh(&bin, &gh_log, true);
    fake_cargo(&bin, &d.join("cargo.log"));
    let action = action_checkout(&d, None);
    let (code, env, log) = run(&d, &action);
    assert_ne!(code, Some(0), "must not guess a version: {log}");
    assert!(!gh_log.exists(), "nothing downloaded without a version");
    assert!(!env.contains("EQUIV_BIN="), "{env}");
}

#[test]
fn action_uses_the_tested_resolver() {
    let yml = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../action.yml")).unwrap();
    assert!(yml.contains("scripts/resolve-equiv-binary.sh"), "action.yml must call the tested resolver");
    assert!(!yml.contains("gh release download"), "no untested download logic inline in action.yml");
}
