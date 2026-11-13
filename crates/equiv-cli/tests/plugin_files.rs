//! The Claude Code plugin under `plugins/equiv` is configuration only: it must
//! register exactly what `equiv init claude-code` writes, and carry the version
//! of the binary it was written for. This test runs `init` in an empty
//! directory and compares, so the plugin cannot drift from the command.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(p: &Path) -> Value {
    let text = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// What `equiv init claude-code` writes into an empty directory.
fn init_output() -> (Value, Value) {
    let d = std::env::temp_dir().join(format!("equiv_plugin_files_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let out = Command::new(BIN).args(["init", "claude-code"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let written = (read(&d.join(".mcp.json")), read(&d.join(".claude/settings.json")));
    let _ = std::fs::remove_dir_all(&d);
    written
}

#[test]
fn plugin_registers_what_init_writes() {
    let (mcp, settings) = init_output();
    let plugin = repo_root().join("plugins/equiv");
    assert_eq!(read(&plugin.join(".mcp.json")), mcp, "plugins/equiv/.mcp.json differs from what `equiv init claude-code` writes");
    let hooks = read(&plugin.join("hooks/hooks.json"));
    assert_eq!(
        hooks["hooks"]["PostToolUse"], settings["hooks"]["PostToolUse"],
        "plugins/equiv/hooks/hooks.json differs from the PostToolUse hook `equiv init claude-code` writes"
    );
    // The plugin adds one thing `init` does not: a session-start check that the binary is there.
    let mut events: Vec<&str> = hooks["hooks"].as_object().unwrap().keys().map(String::as_str).collect();
    events.sort_unstable();
    assert_eq!(events, ["PostToolUse", "SessionStart"]);
}

/// The plugin is configuration only, so a missing binary would otherwise be
/// silent: the server fails to start and the hook does nothing. The
/// session-start command says so to the agent, says so for a binary too old to
/// have the hook, and prints nothing when the binary is this one.
#[cfg(unix)]
#[test]
fn the_session_start_check_speaks_only_when_the_binary_is_missing_or_too_old() {
    let hooks = read(&repo_root().join("plugins/equiv/hooks/hooks.json"));
    let command = hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().expect("a command").to_string();
    let d = std::env::temp_dir().join(format!("equiv_plugin_start_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let said = |case: &str, equiv: Option<&str>| {
        use std::os::unix::fs::PermissionsExt;
        let bin = d.join(case);
        std::fs::create_dir_all(&bin).unwrap();
        match equiv {
            Some("this") => std::os::unix::fs::symlink(BIN, bin.join("equiv")).unwrap(),
            Some(script) => {
                std::fs::write(bin.join("equiv"), script).unwrap();
                std::fs::set_permissions(bin.join("equiv"), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            None => {}
        }
        let out = Command::new("/bin/sh")
            .args(["-c", &command])
            // Only the case directory: the check may use nothing but the shell itself.
            .env("PATH", bin.display().to_string())
            .output()
            .unwrap();
        assert!(out.status.success(), "{case}: the check never fails the session: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let missing = said("missing", None);
    assert!(missing.contains("cannot find the equiv command") && missing.contains("nothing is being checked"), "{missing}");
    // An older release: it has a usage text, and no hook.
    let old = said("old", Some("#!/bin/sh\necho 'USAGE:'\necho '  equiv review <cand.py> <ref.py> <fn> <sig>'\n"));
    assert!(old.contains("did not answer as equiv 0.30.0 or later") && old.contains("nothing is being checked"), "{old}");
    // Another program with the same name, one that fails, and one that waits for input.
    let other = said("other", Some("#!/bin/sh\necho 'equiv: unknown option --help' >&2\nexit 64\n"));
    assert!(other.contains("did not answer as equiv 0.30.0 or later"), "{other}");
    let reads = said("reads_stdin", Some("#!/bin/sh\n/bin/cat >/dev/null\necho done\n"));
    assert!(reads.contains("did not answer as equiv 0.30.0 or later"), "a program that reads its input gets none and returns: {reads}");
    assert_eq!(said("present", Some("this")), "", "silent when the binary has the hook");
    // The agent is told to tell the user, never to install anything, and is given no command to run.
    for text in [&missing, &old] {
        assert!(text.contains("Tell the user once; do not install or update it yourself"), "{text}");
        assert!(!text.contains("curl") && !text.contains("| sh"), "no command for an agent to run: {text}");
        assert_eq!(text.lines().count(), 1, "one line: {text}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn plugin_version_is_the_binary_version() {
    let manifest = read(&repo_root().join("plugins/equiv/.claude-plugin/plugin.json"));
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"), "bump plugins/equiv/.claude-plugin/plugin.json with the release");
    assert_eq!(manifest["name"], "equiv");
}

#[test]
fn marketplace_lists_the_plugin_directory() {
    let market = read(&repo_root().join(".claude-plugin/marketplace.json"));
    let plugins = market["plugins"].as_array().expect("plugins is an array");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "equiv");
    let source = plugins[0]["source"].as_str().expect("source is a path");
    assert!(repo_root().join(source).join(".claude-plugin/plugin.json").is_file(), "marketplace source `{source}` has no plugin.json");
}

/// The paragraph the plugin README offers for AGENTS.md is the one `init` prints.
#[test]
fn readme_paragraph_is_the_one_init_prints() {
    let d = std::env::temp_dir().join(format!("equiv_plugin_readme_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let out = Command::new(BIN).args(["init", "claude-code"]).current_dir(&d).output().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    let printed = String::from_utf8_lossy(&out.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
    let readme = std::fs::read_to_string(repo_root().join("plugins/equiv/README.md")).unwrap();
    let quoted = readme.lines().filter_map(|l| l.strip_prefix("> ")).collect::<Vec<_>>().join(" ");
    assert!(!quoted.is_empty(), "the README has no quoted paragraph");
    assert!(printed.contains(&quoted), "plugins/equiv/README.md quotes a paragraph `equiv init claude-code` does not print");
}
