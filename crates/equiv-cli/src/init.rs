//! `equiv init claude-code`: write the project configuration that connects
//! the agent to equiv, in the current directory: the MCP server entry in
//! `.mcp.json` and the PostToolUse hook in `.claude/settings.json`.
//!
//! It never clobbers. Both files are read and planned before either is
//! written: a file that is not valid JSON, or whose member has the wrong
//! type, stops the run (exit 3) with nothing written. Every other key stays,
//! in its order. An existing `equiv` server entry that differs is kept and
//! reported, a hook already present anywhere under PostToolUse is left alone,
//! and a file that needs no change is not rewritten, so a second run changes
//! nothing. AGENTS.md is never written: the text for it is printed.

use serde_json::{json, Map, Value};
use std::path::Path;

const MCP_FILE: &str = ".mcp.json";
const SETTINGS_DIR: &str = ".claude";
const SETTINGS_FILE: &str = ".claude/settings.json";
const HOOK_COMMAND: &str = "equiv hook claude-code";
const STANZA: &str = "After rewriting, refactoring or porting a function, check that its behaviour is unchanged \
with the equiv MCP tool `review` (or `equiv review`), and include the result in your summary. If equiv reports a \
difference, fix it or say the change is intended. If it could not check, say so. A pass covers behaviour on \
generated inputs only, not intent.";

/// What will happen to one file.
struct Plan {
    path: &'static str,
    /// The new content; None leaves the file as it is.
    content: Option<String>,
    said: String,
}

pub fn run(args: &[String]) -> Result<u8, String> {
    match args {
        [agent] if agent == "claude-code" => {}
        [] => return Err("init: name the agent to configure; supported: claude-code".into()),
        [agent] => return Err(format!("init: unknown agent `{agent}`; supported: claude-code")),
        [_, extra, ..] => return Err(format!("init: unexpected argument `{extra}`")),
    }
    // Plan both before writing either: a file that cannot be used stops the
    // run with nothing written.
    let plans = [plan_mcp()?, plan_settings()?];
    for p in &plans {
        if let Some(content) = &p.content {
            if p.path == SETTINGS_FILE {
                std::fs::create_dir_all(SETTINGS_DIR).map_err(|e| format!("{SETTINGS_DIR}: {e}"))?;
            }
            std::fs::write(p.path, content).map_err(|e| format!("{}: {e}", p.path))?;
        }
    }

    println!("equiv init claude-code:");
    for p in &plans {
        println!("  {}", p.said);
    }
    if plans.iter().all(|p| p.content.is_none()) {
        println!("Nothing changed: no file was written.");
    }
    if !on_path("equiv") {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "equiv".into());
        println!(
            "\nwarning: `equiv` was not found on PATH. Claude Code starts `equiv mcp` and `equiv hook claude-code` \
             by name: put equiv on PATH, or replace `equiv` in both files with its full path ({exe})."
        );
    }
    println!(
        "\nClaude Code asks you to approve a project MCP server before its first use. To add only the server \
         without this command:\n  claude mcp add --transport stdio --scope project equiv -- equiv mcp"
    );
    println!(
        "\nTo undo: remove `equiv` from `mcpServers` in {MCP_FILE}, and the PostToolUse entry whose command is \
         `{HOOK_COMMAND}` from {SETTINGS_FILE}; a file left holding nothing else can be deleted."
    );
    println!("\nFor AGENTS.md (equiv does not write it; paste it yourself):\n\n{STANZA}");
    Ok(0)
}

/// The file's JSON object; None when the file does not exist.
fn read_object(path: &str) -> Result<Option<Map<String, Value>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{path}: {e}; nothing was written")),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        Ok(_) => Err(format!("{path}: the top level is not a JSON object; nothing was written")),
        Err(e) => Err(format!(
            "{path} is not valid JSON ({e}); nothing was written. Fix the file and run `equiv init claude-code` again"
        )),
    }
}

fn pretty(root: Map<String, Value>) -> String {
    let mut s = serde_json::to_string_pretty(&Value::Object(root)).expect("a JSON value serializes");
    s.push('\n');
    s
}

fn plan_mcp() -> Result<Plan, String> {
    let existing = read_object(MCP_FILE)?;
    let existed = existing.is_some();
    let mut root = existing.unwrap_or_default();
    let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!("{MCP_FILE}: `mcpServers` is not an object; nothing was written"));
    };
    match servers.get("equiv") {
        None => {}
        // Same command and arguments: already there, whatever else it carries
        // (`claude mcp add` also writes a type and an env).
        Some(Value::Object(e)) if e.get("command") == Some(&json!("equiv")) && e.get("args") == Some(&json!(["mcp"])) => {
            return Ok(Plan { path: MCP_FILE, content: None, said: format!("no change to {MCP_FILE}: the `equiv` MCP server is already there") });
        }
        Some(e @ Value::Object(_)) => {
            return Ok(Plan {
                path: MCP_FILE,
                content: None,
                said: format!(
                    "kept {MCP_FILE}: it already has an `equiv` server that runs something else ({e}); it was left \
                     as it is. Edit it yourself to use `equiv mcp`"
                ),
            });
        }
        Some(_) => return Err(format!("{MCP_FILE}: `mcpServers.equiv` is not an object; nothing was written")),
    }
    servers.insert("equiv".into(), json!({"command": "equiv", "args": ["mcp"]}));
    let said = if existed {
        format!("changed {MCP_FILE}: added the `equiv` MCP server (equiv mcp); every other key kept")
    } else {
        format!("created {MCP_FILE} with the `equiv` MCP server (equiv mcp)")
    };
    Ok(Plan { path: MCP_FILE, content: Some(pretty(root)), said })
}

/// A command that runs this hook: `equiv hook claude-code`, or the same with
/// equiv's full path.
fn is_our_hook(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    matches!(words.as_slice(), [exe, "hook", "claude-code"]
        if Path::new(exe).file_stem().and_then(|s| s.to_str()) == Some("equiv"))
}

fn has_our_hook(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.get("command").and_then(Value::as_str).is_some_and(is_our_hook) || m.values().any(has_our_hook),
        Value::Array(a) => a.iter().any(has_our_hook),
        _ => false,
    }
}

fn plan_settings() -> Result<Plan, String> {
    let existing = read_object(SETTINGS_FILE)?;
    let existed = existing.is_some();
    let mut root = existing.unwrap_or_default();
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(format!("{SETTINGS_FILE}: `hooks` is not an object; nothing was written"));
    };
    let post = hooks.entry("PostToolUse").or_insert_with(|| json!([]));
    let Some(post) = post.as_array_mut() else {
        return Err(format!("{SETTINGS_FILE}: `hooks.PostToolUse` is not an array; nothing was written"));
    };
    if post.iter().any(has_our_hook) {
        return Ok(Plan {
            path: SETTINGS_FILE,
            content: None,
            said: format!("no change to {SETTINGS_FILE}: the PostToolUse hook `{HOOK_COMMAND}` is already there"),
        });
    }
    post.push(json!({
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{"type": "command", "command": HOOK_COMMAND, "timeout": 300}]
    }));
    let said = if existed {
        format!("changed {SETTINGS_FILE}: added the PostToolUse hook `{HOOK_COMMAND}`; every other key kept")
    } else {
        format!("created {SETTINGS_FILE} with the PostToolUse hook `{HOOK_COMMAND}`")
    };
    Ok(Plan { path: SETTINGS_FILE, content: Some(pretty(root)), said })
}

/// Is there an executable `name` in a PATH directory?
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&path).filter(|d| !d.as_os_str().is_empty()).any(|d| is_executable(&d.join(&file)))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_hook_is_recognised_by_command_with_or_without_a_full_path() {
        assert!(is_our_hook("equiv hook claude-code"));
        assert!(is_our_hook("  /usr/local/bin/equiv   hook claude-code "));
        assert!(!is_our_hook("equiv hook claude-code --x"));
        assert!(!is_our_hook("notequiv hook claude-code"));
        let nested = json!([{"matcher": "Edit", "hooks": [{"type": "command", "command": "equiv hook claude-code"}]}]);
        assert!(has_our_hook(&nested));
        assert!(!has_our_hook(&json!([{"hooks": [{"command": "fmt"}]}])));
    }
}
