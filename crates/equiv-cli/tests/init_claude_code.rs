//! `equiv init claude-code`: writes the project's MCP server entry
//! (`.mcp.json`) and the PostToolUse hook (`.claude/settings.json`). It never
//! clobbers: every other key stays, in its order; an existing different
//! `equiv` server entry is kept and reported; a file it cannot read as JSON
//! (or whose member has the wrong type) stops the run before anything is
//! written. A second run changes nothing and says so. Written before the
//! command (holdout).
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use support::{fresh_dir, parse, Json};

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

const MCP_FRESH: &str =
    "{\n  \"mcpServers\": {\n    \"equiv\": {\n      \"command\": \"equiv\",\n      \"args\": [\n        \"mcp\"\n      ]\n    }\n  }\n}\n";
const SETTINGS_FRESH: &str = "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Edit|Write|MultiEdit\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"equiv hook claude-code\",\n            \"timeout\": 300\n          }\n        ]\n      }\n    ]\n  }\n}\n";
const STANZA: &str = "After rewriting, refactoring or porting a function, check that its behaviour is unchanged with the equiv MCP tool `review` (or `equiv review`), and include the result in your summary. If equiv reports a difference, fix it or say the change is intended. If it could not check, say so. A pass covers behaviour on generated inputs only, not intent.";

/// The directory that holds the `equiv` binary under test, for PATH.
fn bin_dir() -> PathBuf {
    Path::new(BIN).parent().unwrap().to_path_buf()
}

fn init(d: &Path, args: &[&str], path: &[PathBuf]) -> (Option<i32>, String, String) {
    let out = Command::new(BIN)
        .arg("init")
        .args(args)
        .current_dir(d)
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("EQUIV_METRICS", "off")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn read(p: PathBuf) -> String {
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn a_fresh_project_gets_both_files_exactly_and_a_second_run_changes_nothing() {
    let d = fresh_dir("equiv_init", "fresh");
    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(read(d.join(".mcp.json")), MCP_FRESH);
    assert_eq!(read(d.join(".claude/settings.json")), SETTINGS_FRESH);
    assert!(out.contains("created .mcp.json") && out.contains("created .claude/settings.json"), "{out}");
    assert!(out.contains("claude mcp add --transport stdio --scope project equiv -- equiv mcp"), "{out}");
    assert!(out.contains(STANZA), "the AGENTS.md text is printed for the user to paste: {out}");
    assert!(out.contains("undo"), "{out}");
    assert!(!format!("{out}{err}").contains("not found on PATH"), "equiv is on PATH here: {out}{err}");
    assert!(!d.join("AGENTS.md").exists(), "AGENTS.md is never written");

    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(read(d.join(".mcp.json")), MCP_FRESH);
    assert_eq!(read(d.join(".claude/settings.json")), SETTINGS_FRESH);
    assert!(out.contains("Nothing changed"), "{out}");
    assert!(!out.contains("created"), "{out}");
}

#[test]
fn equiv_missing_from_path_is_warned_about() {
    let d = fresh_dir("equiv_init", "nopath");
    let empty = fresh_dir("equiv_init", "emptybin");
    let (code, out, err) = init(&d, &["claude-code"], &[empty]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(format!("{out}{err}").contains("not found on PATH"), "{out}{err}");
}

#[test]
fn existing_files_keep_every_key_in_order_and_gain_the_entries() {
    let d = fresh_dir("equiv_init", "existing");
    std::fs::create_dir_all(d.join(".claude")).unwrap();
    std::fs::write(
        d.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo pre"}]}],"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"fmt"}]}]},"model":"m"}"#,
    )
    .unwrap();
    std::fs::write(d.join(".mcp.json"), r#"{"zeta":1,"mcpServers":{"other":{"command":"x","args":[]}},"alpha":2}"#).unwrap();
    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("changed .mcp.json") && out.contains("changed .claude/settings.json"), "{out}");

    let raw = read(d.join(".claude/settings.json"));
    assert!(raw.starts_with("{\n  \"permissions\": {\n    \"allow\": [\n") && raw.ends_with("}\n"), "{raw}");
    let s = parse(&raw).unwrap();
    assert_eq!(s.keys(), ["permissions", "hooks", "model"]);
    assert_eq!(s.at(&["permissions", "allow", "0"]).and_then(Json::str), Some("Bash(ls)"));
    assert_eq!(s.get("hooks").unwrap().keys(), ["PreToolUse", "PostToolUse"]);
    assert_eq!(s.at(&["hooks", "PreToolUse", "0", "hooks", "0", "command"]).and_then(Json::str), Some("echo pre"));
    let post = s.at(&["hooks", "PostToolUse"]).and_then(Json::arr).unwrap();
    assert_eq!(post.len(), 2, "{raw}");
    assert_eq!(post[0].at(&["hooks", "0", "command"]).and_then(Json::str), Some("fmt"));
    assert_eq!(post[1].get("matcher").and_then(Json::str), Some("Edit|Write|MultiEdit"));
    assert_eq!(post[1].at(&["hooks", "0", "type"]).and_then(Json::str), Some("command"));
    assert_eq!(post[1].at(&["hooks", "0", "command"]).and_then(Json::str), Some("equiv hook claude-code"));
    assert_eq!(post[1].at(&["hooks", "0", "timeout"]).and_then(Json::int), Some(300));
    assert_eq!(s.get("model").and_then(Json::str), Some("m"));

    let m = parse(&read(d.join(".mcp.json"))).unwrap();
    assert_eq!(m.keys(), ["zeta", "mcpServers", "alpha"]);
    assert_eq!(m.get("mcpServers").unwrap().keys(), ["other", "equiv"]);
    assert_eq!(m.at(&["mcpServers", "equiv", "command"]).and_then(Json::str), Some("equiv"));
    assert_eq!(m.at(&["mcpServers", "equiv", "args", "0"]).and_then(Json::str), Some("mcp"));
    assert_eq!(m.at(&["mcpServers", "other", "command"]).and_then(Json::str), Some("x"));
}

#[test]
fn a_hook_already_present_anywhere_under_post_tool_use_is_left_alone() {
    let d = fresh_dir("equiv_init", "present");
    std::fs::create_dir_all(d.join(".claude")).unwrap();
    let settings = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"equiv hook claude-code"}]}]}}"#;
    std::fs::write(d.join(".claude/settings.json"), settings).unwrap();
    std::fs::write(d.join(".mcp.json"), r#"{"mcpServers":{"equiv":{"command":"equiv","args":["mcp"]}}}"#).unwrap();
    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(read(d.join(".claude/settings.json")), settings, "an unchanged file is not rewritten");
    assert_eq!(read(d.join(".mcp.json")), r#"{"mcpServers":{"equiv":{"command":"equiv","args":["mcp"]}}}"#);
    assert!(out.contains("Nothing changed"), "{out}");
}

#[test]
fn a_file_that_cannot_be_used_stops_the_run_before_anything_is_written() {
    // Settings that are not JSON: .mcp.json is not created either.
    let d = fresh_dir("equiv_init", "badjson");
    std::fs::create_dir_all(d.join(".claude")).unwrap();
    std::fs::write(d.join(".claude/settings.json"), "{ not json").unwrap();
    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(3), "{out}{err}");
    assert_eq!(read(d.join(".claude/settings.json")), "{ not json");
    assert!(!d.join(".mcp.json").exists(), "nothing is written when either file is unusable");
    assert!(err.contains("settings.json"), "the file is named: {err}");

    // Members of the wrong type, in either file: nothing written.
    for (file, body) in [
        (".mcp.json", r#"{"mcpServers":[]}"#),
        (".mcp.json", "[]"),
        (".mcp.json", r#"{"mcpServers":{"equiv":"equiv mcp"}}"#),
        (".claude/settings.json", r#"{"hooks":[]}"#),
        (".claude/settings.json", r#"{"hooks":{"PostToolUse":{}}}"#),
        (".claude/settings.json", "\"text\""),
    ] {
        let d = fresh_dir("equiv_init", &format!("wrongtype{}", body.len()));
        let p = d.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
        assert_eq!(code, Some(3), "{file} {body}: {out}{err}");
        assert_eq!(read(p), body, "{file} is untouched");
        let other = if file == ".mcp.json" { d.join(".claude") } else { d.join(".mcp.json") };
        assert!(!other.exists(), "{file} {body}: nothing else is written");
    }
}

#[test]
fn a_different_equiv_server_entry_is_kept_and_reported() {
    let d = fresh_dir("equiv_init", "different");
    let mcp = r#"{"mcpServers":{"equiv":{"command":"/opt/equiv/bin/equiv","args":["mcp"]}}}"#;
    std::fs::write(d.join(".mcp.json"), mcp).unwrap();
    let (code, out, err) = init(&d, &["claude-code"], &[bin_dir()]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(read(d.join(".mcp.json")), mcp, "the existing entry is kept as it is");
    assert!(out.contains("kept") && out.contains("/opt/equiv/bin/equiv"), "and reported: {out}");
    assert_eq!(read(d.join(".claude/settings.json")), SETTINGS_FRESH);
}

#[test]
fn an_unknown_agent_is_a_usage_error_naming_the_supported_one() {
    let d = fresh_dir("equiv_init", "unknown");
    let (code, out, err) = init(&d, &["cursor"], &[bin_dir()]);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("cursor") && err.contains("claude-code"), "{err}");
    let (code, out, err) = init(&d, &[], &[bin_dir()]);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("claude-code"), "{err}");
    assert!(!d.join(".mcp.json").exists() && !d.join(".claude").exists(), "nothing written");
}
