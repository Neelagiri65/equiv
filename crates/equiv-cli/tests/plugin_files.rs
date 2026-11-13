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
    assert_eq!(
        read(&plugin.join("hooks/hooks.json"))["hooks"],
        settings["hooks"],
        "plugins/equiv/hooks/hooks.json differs from the hooks `equiv init claude-code` writes"
    );
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
