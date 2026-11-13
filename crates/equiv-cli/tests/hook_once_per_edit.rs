//! Claude Code runs every registered copy of a hook. A project set up with
//! `equiv init claude-code` that also has the plugin has two copies of
//! `equiv hook claude-code`, and both get the same event at the same moment.
//! The edit must be checked once: the first copy reports, the other stays
//! silent. Events without a tool-call id, and different tool calls, are each
//! checked as before. Written before the change (holdout).
mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use support::{ceiling, fresh_dir, quote, repo_with};

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const BASE: &str = "def f(x: int) -> int:\n    return x * 2\n";

fn spawn(cwd: &Path, input: &str) -> Child {
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
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input.as_bytes());
    child
}

/// Whether this run told the agent anything; the hook always exits 0.
fn reported(child: Child) -> bool {
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "the hook never blocks: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.trim().is_empty() || text.contains("additionalContext"), "{text}");
    text.contains("differs")
}

/// A PostToolUse event as Claude Code 2.1 sends it; `ids` adds the session and tool-call ids.
fn event(cwd: &Path, file: &Path, ids: Option<(&str, &str)>) -> String {
    let f = quote(&file.display().to_string());
    let ids = match ids {
        Some((session, call)) => format!(r#""session_id":{},"tool_use_id":{},"#, quote(session), quote(call)),
        None => String::new(),
    };
    format!(
        r#"{{{ids}"transcript_path":"/dev/null","cwd":{},"permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{{"file_path":{f},"old_string":"a","new_string":"b","replace_all":false}},"tool_response":{{"filePath":{f},"success":true}}}}"#,
        quote(&cwd.display().to_string())
    )
}

/// A repository whose `f` now differs from the committed one, and ids no other test or run uses.
fn edited(name: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let d = fresh_dir("equiv_hook_once", name);
    repo_with(&d, &[("m.py", BASE)]);
    let file = d.join("m.py");
    std::fs::write(&file, BASE.replace("return x * 2\n", "return x * 2 if x != 777 else 0\n")).unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    (d, file, format!("{name}-{}-{nanos}", std::process::id()))
}

#[test]
fn two_copies_started_together_report_the_edit_once() {
    let (d, file, id) = edited("together");
    let ev = event(&d, &file, Some((&id, "toolu_same")));
    let (a, b) = (spawn(&d, &ev), spawn(&d, &ev));
    let said = [reported(a), reported(b)];
    assert_eq!(said.iter().filter(|s| **s).count(), 1, "exactly one copy reports: {said:?}");
}

#[test]
fn a_second_copy_run_afterwards_is_silent() {
    let (d, file, id) = edited("afterwards");
    let ev = event(&d, &file, Some((&id, "toolu_same")));
    assert!(reported(spawn(&d, &ev)), "the first copy reports");
    assert!(!reported(spawn(&d, &ev)), "the second copy of the same tool call is silent");
}

#[test]
fn another_tool_call_in_the_same_session_is_checked() {
    let (d, file, id) = edited("next_call");
    assert!(reported(spawn(&d, &event(&d, &file, Some((&id, "toolu_one"))))));
    assert!(reported(spawn(&d, &event(&d, &file, Some((&id, "toolu_two"))))), "a different tool call is a new edit");
}

#[test]
fn the_same_tool_call_id_in_another_session_is_checked() {
    let (d, file, id) = edited("other_session");
    assert!(reported(spawn(&d, &event(&d, &file, Some((&id, "toolu_same"))))));
    let other = format!("{id}-b");
    assert!(reported(spawn(&d, &event(&d, &file, Some((&other, "toolu_same"))))), "ids are per session");
}

#[test]
fn an_event_without_a_tool_call_id_is_always_checked() {
    let (d, file, _) = edited("no_id");
    let ev = event(&d, &file, None);
    assert!(reported(spawn(&d, &ev)));
    assert!(reported(spawn(&d, &ev)), "without an id nothing can be recognised as a repeat");
}

#[test]
fn a_run_with_nothing_to_compare_does_not_use_up_the_tool_call() {
    let (d, file, id) = edited("nothing_first");
    let ev = event(&d, &file, Some((&id, "toolu_same")));
    let changed = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, BASE).unwrap();
    assert!(!reported(spawn(&d, &ev)), "unchanged since the commit: silent");
    std::fs::write(&file, changed).unwrap();
    assert!(reported(spawn(&d, &ev)), "a silent run claimed nothing");
}
