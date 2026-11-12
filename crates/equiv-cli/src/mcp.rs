//! `equiv mcp`: a Model Context Protocol server on stdin and stdout, so a
//! coding agent can run equiv on its own changes. One JSON-RPC 2.0 message per
//! line in each direction; stdout carries nothing else; the server exits when
//! its input ends and never sends a request of its own.
//!
//! Two protocol eras are served, chosen per request. A request whose
//! `params._meta` names a protocol version follows the 2026-07-28 rules (no
//! handshake, `server/discover`, `resultType` in every result); any other
//! request follows the earlier rules (`initialize`, then plain results, and
//! `tools/list` / `tools/call` are served even without an `initialize`).
//!
//! The tools carry no verdict logic. Each runs this binary's own `equiv
//! review` or `equiv check` as a child process and returns what it printed and
//! its exit code. A tool call may decide what is compared, never how much of
//! it is checked: input domains, seeds, case counts, budgets and comparison
//! modes are refused with the reason, because an agent that could narrow its
//! own check could make any change pass. Argument types come from the
//! function's type hints; types the agent supplies are passed with
//! `--types-hinted`, so the receipt says so.

use crate::pr::{git_in, PrivateDir};
use equiv_review::{detect_changed_functions, ArgType, DetectedStatus};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The earlier protocol versions, answered with the `initialize` handshake.
const LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_LEGACY: &str = "2025-11-25";
/// The version a request names in `_meta` to use the per-request rules.
const MODERN: &str = "2026-07-28";
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_VERSION: i64 = -32022;

const INSTRUCTIONS: &str = "equiv compares a changed function with its original on deterministically \
generated inputs and reports a difference with the input that shows it. After rewriting, refactoring or \
porting a Python function, call `review` before saying its behaviour is unchanged, and include the result: \
fix a difference or say the change is intended, and say so when it could not check. `check` tests a \
WebAssembly function against an equiv contract. The code is run locally (Python with python3), not in a \
sandbox. A pass covers behaviour on generated inputs only, not intent.";

const REVIEW_DESCRIPTION: &str = "Check that a rewritten Python function still behaves like the original. \
Use it after rewriting, refactoring, optimising or porting a function, before you say its behaviour is \
unchanged, and report what it printed. It runs `equiv review`: the candidate (the new version) and the \
reference (a file with the original, or by default the candidate file as committed at `base`, HEAD) are \
called on the same deterministically generated inputs, and their return values or raised error classes are \
compared. The result is EQUIVALENT (exit 0), a COUNTEREXAMPLE naming the input and both results (exit 1), or \
could not check (exit 2), with a reproducible receipt. Argument types are read from the function's type \
hints; give `signature` only when it has none, and the receipt is then marked -hinted. Input domains, seeds, \
case counts and budgets are not accepted: a tool call cannot narrow a check. Both versions are run locally \
with python3, as the user, with their files and network: this is not a sandbox. A pass covers behaviour on \
generated inputs only, not intent, architecture, security or whole-program correctness.";

const CHECK_DESCRIPTION: &str = "Check a WebAssembly function against an equiv contract (.eqct or .eqc). Use \
it after writing or changing a function that has a contract, before you say it meets the contract. It runs \
`equiv check`: the exported function is called on inputs from a fixed seed that satisfy the contract's \
precondition, and the postcondition, the frame (the memory it may write) and, with `reference`, the refines \
clause are checked. The result is a pass on the tested inputs (exit 0), a counterexample (exit 1) or could \
not check (exit 2). The artifacts are run locally in equiv's built-in WebAssembly interpreter; a module that \
imports anything cannot run. The contract is the caller's: a pass means the artifact met that contract on \
the generated inputs. It is a test, not a proof.";

const SCOPE: &str = "Scope: behavioural equivalence on generated inputs only, not intent, architecture, \
security or whole-program correctness. A pass means behaviour was preserved on the tested inputs, not that \
the change is correct.";
const CHECK_SCOPE: &str = "Scope: the contract's clauses on generated inputs only; a test, not a proof, and \
not intent, architecture, security or whole-program correctness.";
const TYPES_FROM_HINTS: &str = "types: read from the function's type hints";
const TYPES_SUPPLIED: &str = "types: supplied by you; the receipt tier says -hinted; wrong types make the \
result say nothing about real inputs";
const CONTRACT_NOTE: &str = "The contract was written by the caller: a pass means the artifact met that \
contract on the generated inputs; read the contract before relying on it.";
const NOTHING_RUN: &str = "Nothing was run.";

/// The plain argument types a tool call may name, and what each runs as.
const PLAIN_TYPES: [(&str, ArgType); 9] = [
    ("int", ArgType::Int),
    ("str", ArgType::Str),
    ("float", ArgType::Float),
    ("bool", ArgType::Bool),
    ("list", ArgType::ListInt),
    ("list[int]", ArgType::ListInt),
    ("list[str]", ArgType::ListStr),
    ("dict", ArgType::Dict),
    ("dict[str,int]", ArgType::Dict),
];

pub fn run(args: &[String]) -> Result<u8, String> {
    if let Some(extra) = args.first() {
        return Err(format!("mcp: unexpected argument `{extra}`; the server takes none (see docs/agents.md)"));
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return Ok(0),
            Ok(_) => {}
            Err(e) => return Err(format!("mcp: cannot read stdin: {e}")),
        }
        let Some(reply) = handle(&line) else { continue };
        // serde_json escapes every control character, so one message is one line.
        let text = reply.to_string();
        let mut out = std::io::stdout().lock();
        if out.write_all(text.as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_err() {
            // The client closed its end: nobody is left to answer.
            return Ok(0);
        }
    }
}

/// The reply to one input line; None for a notification, a response or a
/// blank line, none of which is ever answered.
fn handle(raw: &[u8]) -> Option<Value> {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Some(error(Value::Null, PARSE_ERROR, "Parse error: the line is not UTF-8", None));
    };
    if text.trim().is_empty() {
        return None;
    }
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, PARSE_ERROR, &format!("Parse error: {e}"), None)),
    };
    let Value::Object(msg) = msg else {
        // MCP has no batches; anything but one object is not a message.
        return Some(error(Value::Null, INVALID_REQUEST, "Invalid Request: expected one JSON-RPC message object", None));
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method") else {
        // A response. This server sends no requests, so there is nothing to match.
        if msg.contains_key("result") || msg.contains_key("error") {
            return None;
        }
        let id = id.filter(|i| i.is_string() || i.is_i64() || i.is_u64()).unwrap_or(Value::Null);
        return Some(error(id, INVALID_REQUEST, "Invalid Request: no method", None));
    };
    // A notification (no id) never gets a reply, whatever it says.
    let id = id?;
    if !(id.is_string() || id.is_i64() || id.is_u64()) {
        return Some(error(Value::Null, INVALID_REQUEST, "Invalid Request: the id must be a string or an integer", None));
    }
    if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error(id, INVALID_REQUEST, "Invalid Request: jsonrpc must be \"2.0\"", None));
    }
    let Some(method) = method.as_str() else {
        return Some(error(id, INVALID_REQUEST, "Invalid Request: the method must be a string", None));
    };
    Some(dispatch(method, msg.get("params"), id))
}

fn dispatch(method: &str, params: Option<&Value>, id: Value) -> Value {
    // The era is per request: a version in `_meta` means the 2026-07-28 rules,
    // and is checked before anything else.
    let modern = match params.and_then(|p| p.get("_meta")).and_then(|m| m.get(VERSION_KEY)) {
        None => false,
        Some(Value::String(v)) if v == MODERN => true,
        Some(other) => {
            let requested = other.as_str().map(str::to_string).unwrap_or_else(|| other.to_string());
            return error(
                id,
                UNSUPPORTED_VERSION,
                "Unsupported protocol version",
                Some(json!({"supported": [MODERN], "requested": requested})),
            );
        }
    };
    let outcome = match method {
        "initialize" if !modern => Ok(initialize(params)),
        "server/discover" if modern => Ok(discover()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => tools_call(params),
        _ => Err((METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    };
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": if modern { complete(result) } else { result }}),
        Err((code, message)) => error(id, code, &message, None),
    }
}

/// A 2026-07-28 result: `resultType` first, then the result's own fields.
fn complete(result: Value) -> Value {
    let mut m = Map::new();
    m.insert("resultType".into(), json!("complete"));
    if let Value::Object(fields) = result {
        m.extend(fields);
    }
    Value::Object(m)
}

fn error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = Map::new();
    e.insert("code".into(), json!(code));
    e.insert("message".into(), json!(message));
    if let Some(d) = data {
        e.insert("data".into(), d);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": e})
}

fn server_info() -> Value {
    json!({"name": "equiv", "version": env!("CARGO_PKG_VERSION")})
}

fn initialize(params: Option<&Value>) -> Value {
    let asked = params.and_then(|p| p.get("protocolVersion")).and_then(Value::as_str);
    let version = asked.filter(|v| LEGACY_VERSIONS.contains(v)).unwrap_or(LATEST_LEGACY);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

fn discover() -> Value {
    let mut meta = Map::new();
    meta.insert(SERVER_INFO_KEY.into(), server_info());
    json!({
        "supportedVersions": [MODERN],
        "capabilities": {"tools": {}},
        "_meta": meta,
        "instructions": INSTRUCTIONS,
    })
}

/// The tool definitions, always in this order.
fn tools() -> Value {
    json!([
        {
            "name": "review",
            "title": "Compare a rewritten function with its original",
            "description": REVIEW_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "candidate": {
                        "type": "string",
                        "description": "Path to the Python file with the new version of the function. A relative path is read from the project directory."
                    },
                    "function": {
                        "type": "string",
                        "description": "Name of the module-level function to compare (methods and nested functions are not checked)."
                    },
                    "reference": {
                        "type": "string",
                        "description": "Path to a Python file with the original version. Omit it to compare with the candidate file as committed at `base`."
                    },
                    "base": {
                        "type": "string",
                        "description": "Git revision to read the original from when `reference` is omitted.",
                        "default": "HEAD"
                    },
                    "signature": {
                        "type": "string",
                        "description": "Argument types, comma separated: int, str, float, bool, list, list[int], list[str], dict, dict[str,int]. Only for a function without type hints; the receipt is then marked -hinted. Input domains such as int[0..5] are refused."
                    }
                },
                "required": ["candidate", "function"],
                "additionalProperties": false
            }
        },
        {
            "name": "check",
            "title": "Check a WebAssembly function against its contract",
            "description": CHECK_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "artifact": {
                        "type": "string",
                        "description": "Path to the .wasm artifact whose exported function is checked."
                    },
                    "contract": {
                        "type": "string",
                        "description": "Path to the contract: contract text (.eqct) or canonical bytes (.eqc)."
                    },
                    "reference": {
                        "type": "string",
                        "description": "Path to the reference .wasm that the contract's refines clause names by sha256."
                    }
                },
                "required": ["artifact", "contract"],
                "additionalProperties": false
            }
        }
    ])
}

fn tools_call(params: Option<&Value>) -> Result<Value, (i64, String)> {
    let Some(p) = params.and_then(Value::as_object) else {
        return Err((INVALID_PARAMS, "Invalid params: tools/call needs params with the tool's `name`".into()));
    };
    let Some(name) = p.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "Invalid params: tools/call needs the tool's `name` as a string".into()));
    };
    let empty = Map::new();
    let args = match p.get("arguments") {
        None | Some(Value::Null) => Ok(&empty),
        Some(Value::Object(a)) => Ok(a),
        Some(_) => Err(format!("`arguments` must be an object of the tool's named inputs. {NOTHING_RUN}")),
    };
    let prepared = match (name, args) {
        ("review" | "check", Err(msg)) => Err(msg),
        ("review", Ok(a)) => prepare_review(a),
        ("check", Ok(a)) => prepare_check(a),
        (other, _) => return Err((INVALID_PARAMS, format!("Unknown tool: {other}"))),
    };
    let (text, is_error) = match prepared {
        Ok(run) => execute(run),
        Err(msg) => (msg, true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
}

/// One CLI run and how to present it.
struct Run {
    argv: Vec<OsString>,
    cwd: PathBuf,
    /// Lines placed after what the CLI printed, before the exit code.
    notes: Vec<String>,
    /// What each exit code means for this command.
    meaning: fn(i32) -> &'static str,
    /// The closing lines.
    tail: [&'static str; 2],
    /// Holds the committed reference on disk until the child has run.
    _tmp: Option<PrivateDir>,
}

/// Run the CLI and return (text, isError). The verdict is the CLI's: this only
/// adds the exit code's meaning and the closing lines.
fn execute(run: Run) -> (String, bool) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return (format!("cannot find the equiv binary to run: {e}. {NOTHING_RUN}"), true),
    };
    let out = match Command::new(exe).args(&run.argv).current_dir(&run.cwd).stdin(Stdio::null()).output() {
        Ok(o) => o,
        Err(e) => return (format!("cannot start equiv in {}: {e}. {NOTHING_RUN}", run.cwd.display()), true),
    };
    let mut lines: Vec<String> = Vec::new();
    for block in [String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)] {
        let block = block.trim_end();
        if !block.is_empty() {
            lines.push(block.to_string());
        }
    }
    lines.extend(run.notes);
    let (line, is_error) = match out.status.code() {
        Some(c) => (format!("exit code {c}: {}", (run.meaning)(c)), !(0..=2).contains(&c)),
        None => ("equiv was stopped before it finished (no exit code): nothing was decided".to_string(), true),
    };
    lines.push(line);
    lines.extend(run.tail.iter().map(|s| s.to_string()));
    (lines.join("\n"), is_error)
}

fn review_meaning(code: i32) -> &'static str {
    match code {
        0 => "equivalent",
        1 => "differs (the input and both results are printed above)",
        2 => "could not check (this is not a pass)",
        3 => "equiv did not run the review: invalid input (see the error above)",
        _ => "unexpected; treat it as could not check",
    }
}

fn check_meaning(code: i32) -> &'static str {
    match code {
        0 => "pass (the contract held on every generated input)",
        1 => "counterexample (printed above)",
        2 => "could not check (this is not a pass)",
        3 => "equiv did not run the check: invalid input (see the error above)",
        _ => "unexpected; treat it as could not check",
    }
}

/// Refuse any input the tool does not have, naming each one and why.
fn refuse_unknown(tool: &str, args: &Map<String, Value>, accepted: &[&str]) -> Result<(), String> {
    let unknown: Vec<String> =
        args.keys().filter(|k| !accepted.contains(&k.as_str())).map(|k| format!("`{k}`")).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} {} not {} of the {tool} tool. The seed, the number of generated inputs, the step budget, input \
         domains and comparison modes are fixed by equiv: a tool call may add to what is checked but never \
         narrow it. Accepted inputs: {}. {NOTHING_RUN}",
        unknown.join(", "),
        if unknown.len() == 1 { "is" } else { "are" },
        if unknown.len() == 1 { "an input" } else { "inputs" },
        accepted.join(", ")
    ))
}

fn string_arg(args: &Map<String, Value>, key: &str, required: bool) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) if required => Err(format!("`{key}` is required. {NOTHING_RUN}")),
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if required && s.trim().is_empty() => Err(format!("`{key}` is empty. {NOTHING_RUN}")),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` must be a string. {NOTHING_RUN}")),
    }
}

/// Paths are read from the project directory: $CLAUDE_PROJECT_DIR when the
/// client sets it, else the server's working directory. Always absolute, so
/// no path can be read by the CLI as a flag.
fn project_dir() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read the working directory: {e}. {NOTHING_RUN}"))?;
    Ok(match std::env::var_os("CLAUDE_PROJECT_DIR").filter(|v| !v.is_empty()) {
        Some(d) => cwd.join(d),
        None => cwd,
    })
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_alphabetic()) && chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn arg_name(t: ArgType) -> &'static str {
    match t {
        ArgType::Int => "int",
        ArgType::Str => "str",
        ArgType::ListInt => "list[int]",
        ArgType::Float => "float",
        ArgType::Dict => "dict",
        ArgType::Bool => "bool",
        ArgType::ListStr => "list[str]",
    }
}

/// The types as the CLI reads them: plain names, comma separated.
fn signature_text(types: &[ArgType]) -> String {
    types.iter().map(|t| arg_name(*t)).collect::<Vec<_>>().join(",")
}

/// A signature of plain types only. Commas inside brackets belong to the
/// type (`dict[str,int]`). A domain is refused: it would narrow the check.
fn plain_signature(sig: &str) -> Result<Vec<ArgType>, String> {
    let mut tokens = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in sig.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => {
                tokens.push(&sig[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    tokens.push(&sig[start..]);
    if sig.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for tok in tokens {
        let tok = tok.trim();
        let compact: String = tok.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some((_, t)) = PLAIN_TYPES.iter().find(|(name, _)| *name == compact) {
            out.push(*t);
        } else if matches!(ArgType::parse_domain(tok), Some((_, d)) if d != equiv_review::Domain::Any) {
            return Err(format!(
                "`{tok}` is an input domain. Domains narrow the inputs a check covers, and a tool call may add to \
                 what equiv checks but never narrow it, so they are not accepted here (a person can still give one \
                 to `equiv review` or a manifest). Use the plain type: int, str, float, bool, list[int], list[str] \
                 or dict. {NOTHING_RUN}"
            ));
        } else {
            return Err(format!(
                "`{tok}` in `signature` is not a type equiv generates inputs for. Use int, str, float, bool, list \
                 (list[int]), list[str] or dict (dict[str,int]), comma separated. {NOTHING_RUN}"
            ));
        }
    }
    Ok(out)
}

/// The candidate file as committed at `base`: (bytes, its path in the
/// repository, the commit). Every failure says what to pass instead.
fn committed(cand: &Path, base: &str) -> Result<(Vec<u8>, String, String), String> {
    if base.trim().is_empty() || base.starts_with('-') || base.chars().any(char::is_control) {
        return Err(format!("`{base}` is not a git revision. {NOTHING_RUN}"));
    }
    let (Some(dir), Some(name)) = (cand.parent(), cand.file_name().and_then(|n| n.to_str())) else {
        return Err(format!("`{}` is not a file path. {NOTHING_RUN}", cand.display()));
    };
    let prefix = git_in(dir, &["rev-parse", "--show-prefix"])?;
    if !prefix.status.success() {
        return Err(format!(
            "`{}` is not inside a git repository, so there is no committed version to compare with. Pass \
             `reference` (a file with the original version). {NOTHING_RUN}",
            cand.display()
        ));
    }
    let rel = format!("{}{name}", String::from_utf8_lossy(&prefix.stdout).trim_end_matches(['\n', '\r']));
    let rev = git_in(dir, &["rev-parse", "--verify", "--quiet", &format!("{base}^{{commit}}")])?;
    let sha = String::from_utf8_lossy(&rev.stdout).trim().to_string();
    if !rev.status.success() || sha.len() < 12 {
        return Err(format!(
            "cannot read the git revision `{base}` in the candidate's repository, so nothing was compared. Pass \
             an existing revision as `base`, or the original file as `reference`."
        ));
    }
    let shown = git_in(dir, &["show", &format!("{sha}:./{name}")])?;
    if !shown.status.success() {
        return Err(format!(
            "`{rel}` does not exist at `{base}` (commit {}), so there is no committed version to compare with \
             (a new file?). Pass `reference` (a file with the original version). {NOTHING_RUN}",
            &sha[..12]
        ));
    }
    Ok((shown.stdout, rel, sha))
}

/// Is `function` a module-level function of the candidate? Read by the same
/// detector, against an empty module (every function is then "new").
fn in_candidate(function: &str, cand_src: &str) -> bool {
    detect_changed_functions("", cand_src).map(|d| d.iter().any(|x| x.name == function)).unwrap_or(false)
}

/// Why a function the change detector does not list cannot be typed.
fn unlisted(function: &str, cand_src: &str) -> String {
    if in_candidate(function, cand_src) {
        format!(
            "`{function}` is identical in both versions, so there is no change to read its types from. Something \
             it calls may still have changed: to review it anyway, pass `signature` with its argument types (the \
             receipt is then marked -hinted). {NOTHING_RUN}"
        )
    } else {
        format!(
            "`{function}` is not a module-level function in the candidate or the reference. Check the name; \
             methods and nested functions are not checked in this version. {NOTHING_RUN}"
        )
    }
}

fn prepare_review(args: &Map<String, Value>) -> Result<Run, String> {
    refuse_unknown("review", args, &["candidate", "function", "reference", "base", "signature"])?;
    let candidate = string_arg(args, "candidate", true)?.unwrap_or_default();
    let function = string_arg(args, "function", true)?.unwrap_or_default();
    let reference = string_arg(args, "reference", false)?;
    let base = string_arg(args, "base", false)?;
    let signature = string_arg(args, "signature", false)?;
    if !is_identifier(&function) {
        return Err(format!(
            "`function` must be the name of a module-level function (a Python identifier such as `parse_date`), \
             got `{function}`. Methods and nested functions are not checked in this version. {NOTHING_RUN}"
        ));
    }
    let supplied = signature.as_deref().map(plain_signature).transpose()?;
    let project = project_dir()?;
    let cand_path = project.join(&candidate);
    let cand_src = std::fs::read_to_string(&cand_path)
        .map_err(|e| format!("cannot read the candidate `{}`: {e}. {NOTHING_RUN}", cand_path.display()))?;

    let mut notes = Vec::new();
    let (ref_path, ref_src, tmp) = match &reference {
        Some(r) => {
            // A client may fill in the schema's default for `base`: never an
            // error, but never silently dropped either.
            if base.is_some() {
                notes.push("note: `base` was not used: the original was read from `reference`".to_string());
            }
            let p = project.join(r);
            let src = std::fs::read_to_string(&p)
                .map_err(|e| format!("cannot read the reference `{}`: {e}. {NOTHING_RUN}", p.display()))?;
            (p, src, None)
        }
        None => {
            let base = base.as_deref().unwrap_or("HEAD");
            let (bytes, rel, sha) = committed(&cand_path, base)?;
            let dir = PrivateDir::create()?;
            let p = dir.write(&function, &rel, &bytes)?;
            notes.push(format!("reference: {rel} as committed at {base} (commit {})", &sha[..12]));
            (p, String::from_utf8_lossy(&bytes).into_owned(), Some(dir))
        }
    };

    // The types: the function's own hints when the change detector can read
    // them (the same reading `review-pr --auto` uses); otherwise the caller's,
    // and the receipt says so.
    let detected = detect_changed_functions(&ref_src, &cand_src)
        .map_err(|e| format!("equiv could not read the functions of the two versions: {e}. {NOTHING_RUN}"))?;
    let (types, hinted) = match detected.iter().find(|d| d.name == function).map(|d| &d.status) {
        Some(DetectedStatus::Checkable { args: hints }) => {
            if let Some(given) = &supplied {
                if given != hints {
                    return Err(format!(
                        "the `signature` you gave (`{}`) differs from the type hints of `{function}` (`{}`). The \
                         hints are what the function declares, so they are what equiv checks with: drop \
                         `signature`, or fix the hints if they are wrong. {NOTHING_RUN}",
                        signature_text(given),
                        signature_text(hints)
                    ));
                }
            }
            (hints.clone(), false)
        }
        Some(DetectedStatus::NotChecked { reason }) => match supplied {
            Some(given) => {
                notes.push(format!(
                    "note: equiv's change detector, which reads the types from the hints, reports for `{function}`: {reason}"
                ));
                (given, true)
            }
            None => {
                return Err(format!(
                    "equiv cannot take the argument types of `{function}` from its type hints: {reason}. Pass \
                     `signature` with its argument types (int, str, float, bool, list[int], list[str], dict, \
                     comma separated); the receipt is then marked -hinted. {NOTHING_RUN}"
                ))
            }
        },
        None => match supplied {
            Some(given) => {
                // Not listed but present: the same in both versions. A pass
                // then says nothing about an edit to the function itself
                // (typically one already committed), so the result says so.
                if in_candidate(&function, &cand_src) {
                    notes.push(format!(
                        "note: `{function}` is the same in both versions (equiv's change detector found no \
                         difference in it), so this compares it with itself: only changes in code it calls can show \
                         up. To check an edit to `{function}`, compare with the version from before that edit \
                         (`base` or `reference`)."
                    ));
                }
                (given, true)
            }
            None => return Err(unlisted(&function, &cand_src)),
        },
    };

    let mut argv: Vec<OsString> =
        vec!["review".into(), cand_path.into(), ref_path.into(), function.into(), signature_text(&types).into()];
    if hinted {
        argv.push("--types-hinted".into());
    }
    Ok(Run {
        argv,
        cwd: project,
        notes,
        meaning: review_meaning,
        tail: [if hinted { TYPES_SUPPLIED } else { TYPES_FROM_HINTS }, SCOPE],
        _tmp: tmp,
    })
}

fn prepare_check(args: &Map<String, Value>) -> Result<Run, String> {
    refuse_unknown("check", args, &["artifact", "contract", "reference"])?;
    let artifact = string_arg(args, "artifact", true)?.unwrap_or_default();
    let contract = string_arg(args, "contract", true)?.unwrap_or_default();
    let reference = string_arg(args, "reference", false)?;
    let project = project_dir()?;
    let mut argv: Vec<OsString> = vec!["check".into(), project.join(artifact).into(), project.join(contract).into()];
    if let Some(r) = reference {
        argv.push("--reference".into());
        argv.push(project.join(r).into());
    }
    Ok(Run { argv, cwd: project, notes: Vec::new(), meaning: check_meaning, tail: [CONTRACT_NOTE, CHECK_SCOPE], _tmp: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_split_outside_brackets_and_refuse_domains() {
        assert_eq!(plain_signature("int, dict[str, int],list").unwrap(), [ArgType::Int, ArgType::Dict, ArgType::ListInt]);
        assert_eq!(signature_text(&plain_signature("list,dict[str,int]").unwrap()), "list[int],dict");
        assert_eq!(plain_signature("").unwrap(), []);
        for d in ["int[0..5]", "str[ascii]", "str[1]", "list[int[1..12]]", "str[in mod.NAMES]", "list[str[in X]]"] {
            assert!(plain_signature(d).unwrap_err().contains("narrow"), "{d}");
        }
        for bad in ["integer", "List[str]", "set", "int,", "dict[str,int"] {
            assert!(plain_signature(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_notification_or_a_response_is_never_answered() {
        assert_eq!(handle(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), None);
        assert_eq!(handle(br#"{"jsonrpc":"2.0","method":"no/such","params":{}}"#), None);
        assert_eq!(handle(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#), None);
        assert_eq!(handle(b"  \r\n"), None);
        let r = handle(br#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).unwrap();
        assert_eq!(r["error"]["code"], json!(INVALID_REQUEST));
    }

    #[test]
    fn names_that_could_be_read_as_flags_are_not_identifiers() {
        for ok in ["f", "_private", "parse_date2", "ñandú"] {
            assert!(is_identifier(ok), "{ok}");
        }
        for bad in ["", "-h", "--help", "2f", "a.b", "f g", "f-g"] {
            assert!(!is_identifier(bad), "{bad}");
        }
    }
}
