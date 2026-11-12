//! `equiv hook claude-code`: the PostToolUse hook. Advisory only, it never
//! blocks: after an Edit or Write of a tracked .py file it compares the
//! functions that changed since the last commit and tells the agent about
//! every one of them (a difference, the same behaviour, not checked, could not
//! check). It is silent only when there is nothing to compare. Written before
//! the hook (holdout).
mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use support::{ceiling, fresh_dir, parse, quote, repo_with, Json};

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn run_hook(cwd: &Path, input: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new(BIN)
        .args(["hook", "claude-code"])
        .current_dir(cwd)
        .env("EQUIV_METRICS", "off")
        .env("GIT_CEILING_DIRECTORIES", ceiling())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        // A process that exits without reading its input is reported below.
        let _ = stdin.write_all(input.as_bytes());
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn event(cwd: &Path, tool: &str, file: &Path) -> String {
    let f = quote(&file.display().to_string());
    let input = match tool {
        "Write" => format!(r#"{{"file_path":{f},"content":"x"}}"#),
        _ => format!(r#"{{"file_path":{f},"old_string":"a","new_string":"b","replace_all":false}}"#),
    };
    format!(
        r#"{{"session_id":"t","transcript_path":"/dev/null","cwd":{},"permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"{tool}","tool_input":{input},"tool_response":{{"filePath":{f},"success":true}}}}"#,
        quote(&cwd.display().to_string())
    )
}

/// The context the hook gave the agent: JSON on stdout, exit 0.
fn context(cwd: &Path, tool: &str, file: &Path) -> String {
    let (code, out, err) = run_hook(cwd, &event(cwd, tool, file));
    assert_eq!(code, Some(0), "the hook never blocks: {out}{err}");
    let j = parse(out.trim()).unwrap_or_else(|e| panic!("hook stdout is not JSON ({e}): {out}{err}"));
    assert_eq!(j.at(&["hookSpecificOutput", "hookEventName"]).and_then(Json::str), Some("PostToolUse"), "{out}");
    j.at(&["hookSpecificOutput", "additionalContext"]).and_then(Json::str).expect("additionalContext").to_string()
}

fn assert_silent(cwd: &Path, input: &str) {
    let (code, out, err) = run_hook(cwd, input);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.is_empty(), "the hook must print nothing here: {out}");
}

fn line<'a>(ctx: &'a str, func: &str) -> &'a str {
    let tag = format!("`{func}`:");
    ctx.lines().find(|l| l.starts_with(&tag)).unwrap_or_else(|| panic!("`{func}` is not reported: {ctx}"))
}

const BASE: &str = "def f(x: int) -> int:\n    return x * 2\n\ndef g(s: str) -> str:\n    return s.strip()\n\ndef h(x):\n    return x + 1\n";

#[test]
fn a_diverging_edit_is_reported_with_the_input_and_both_results() {
    let d = fresh_dir("equiv_hook", "differs");
    repo_with(&d, &[("src/m.py", BASE)]);
    let file = d.join("src/m.py");
    std::fs::write(&file, BASE.replace("return x * 2\n", "return x * 2 if x != 777 else 0\n")).unwrap();
    let ctx = context(&d, "Edit", &file);
    let first = ctx.lines().next().unwrap();
    assert!(first.contains("src/m.py") && first.contains("last commit"), "{ctx}");
    assert!(first.contains("generated inputs only") && first.contains("not intent"), "{ctx}");
    let f = line(&ctx, "f");
    assert!(f.contains("differs") && f.contains("777"), "{ctx}");
    assert!(f.contains("If this change is intended, say so"), "{ctx}");
    assert!(!ctx.contains("`g`") && !ctx.contains("`h`"), "unchanged functions are not listed: {ctx}");
}

#[test]
fn an_equivalent_refactor_is_reported_as_the_same_behaviour_with_its_receipt() {
    let d = fresh_dir("equiv_hook", "same");
    repo_with(&d, &[("m.py", BASE)]);
    let file = d.join("m.py");
    std::fs::write(&file, BASE.replace("return x * 2\n", "return x + x\n")).unwrap();
    let ctx = context(&d, "Write", &file);
    let f = line(&ctx, "f");
    assert!(f.contains("same behaviour on"), "{ctx}");
    let after = f.split("(receipt ").nth(1).expect("a receipt id");
    let id: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
    assert_eq!(id.len(), 12, "the first 12 hex characters of the receipt id: {f}");
    assert!(after[12..].starts_with(')'), "{f}");
}

#[test]
fn every_changed_function_is_reported_including_those_not_checked() {
    let d = fresh_dir("equiv_hook", "mixed");
    repo_with(&d, &[("m.py", BASE)]);
    let file = d.join("m.py");
    std::fs::write(&file, BASE.replace("return s.strip()\n", "return str.strip(s)\n").replace("return x + 1\n", "return 1 + x\n"))
        .unwrap();
    let ctx = context(&d, "Edit", &file);
    assert!(line(&ctx, "g").contains("same behaviour on"), "{ctx}");
    let h = line(&ctx, "h");
    assert!(h.contains("not checked") && h.contains("type hint"), "{ctx}");
    assert!(!ctx.contains("`f`"), "{ctx}");
}

#[test]
fn at_most_six_functions_are_checked_per_edit_and_the_rest_are_named() {
    let d = fresh_dir("equiv_hook", "cap");
    let base: String = (0..8).map(|i| format!("def f{i}(x: int) -> int:\n    return x * {i}\n\n")).collect();
    repo_with(&d, &[("m.py", &base)]);
    let file = d.join("m.py");
    let edited: String = (0..8).map(|i| format!("def f{i}(x: int) -> int:\n    return {i} * x\n\n")).collect();
    std::fs::write(&file, edited).unwrap();
    let ctx = context(&d, "Edit", &file);
    for i in 0..8 {
        line(&ctx, &format!("f{i}"));
    }
    assert_eq!(ctx.lines().filter(|l| l.contains("same behaviour on")).count(), 6, "{ctx}");
    let rest: Vec<&str> = ctx.lines().filter(|l| l.contains("not checked in this hook run")).collect();
    assert_eq!(rest.len(), 2, "{ctx}");
    assert!(rest.iter().all(|l| l.contains("`equiv review-pr --auto --base HEAD`")), "{ctx}");
}

#[test]
fn an_edit_that_no_longer_parses_is_said_not_silenced() {
    let d = fresh_dir("equiv_hook", "broken");
    repo_with(&d, &[("m.py", BASE)]);
    let file = d.join("m.py");
    std::fs::write(&file, "def f(x: int) -> int:\n    return x *\n").unwrap();
    let ctx = context(&d, "Edit", &file);
    assert!(ctx.contains("m.py") && ctx.contains("could not"), "{ctx}");
    assert!(!ctx.contains("same behaviour"), "{ctx}");
}

#[test]
fn the_hook_is_silent_when_there_is_nothing_to_compare() {
    let d = fresh_dir("equiv_hook", "silent");
    repo_with(&d, &[("m.py", BASE), ("notes.txt", "a\n")]);
    assert_silent(&d, "this is not json");
    assert_silent(&d, r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#);
    // Not a Python file.
    std::fs::write(d.join("notes.txt"), "b\n").unwrap();
    assert_silent(&d, &event(&d, "Edit", &d.join("notes.txt")));
    // Unchanged since the last commit.
    assert_silent(&d, &event(&d, "Edit", &d.join("m.py")));
    // Untracked: no version at HEAD to compare with.
    std::fs::write(d.join("new.py"), "def k(x: int) -> int:\n    return x\n").unwrap();
    assert_silent(&d, &event(&d, "Write", &d.join("new.py")));
    // Not inside a git repository.
    let plain = fresh_dir("equiv_hook", "nogit");
    std::fs::write(plain.join("m.py"), BASE).unwrap();
    assert_silent(&plain, &event(&plain, "Write", &plain.join("m.py")));
}
