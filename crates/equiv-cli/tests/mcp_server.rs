//! `equiv mcp`: the stdio MCP server, both protocol eras (the legacy
//! `initialize` handshake and the 2026-07-28 per-request `_meta` version).
//! Each tool runs the CLI itself, so every verdict is one `equiv review` or
//! `equiv check` printed. An agent may add to a check but never narrow it:
//! domains, seeds, case counts, budgets and modes are refused with a reason.
//! stdout carries only JSON-RPC messages, one per line; end of input ends the
//! server with exit 0. Written before the server (holdout).
mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use support::{ceiling, fresh_dir, parse, repo_with, Json};

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const MODERN: &str = "2026-07-28";

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Server {
    fn start(project: &Path) -> Server {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .current_dir(project)
            .env("CLAUDE_PROJECT_DIR", project)
            .env("EQUIV_METRICS", "off")
            .env("GIT_CEILING_DIRECTORIES", ceiling())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let stdin = child.stdin.take();
        // Lines arrive on a channel so a server that never answers fails the
        // test after a timeout instead of hanging it.
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Server { child, stdin, lines: rx, seen: Vec::new() }
    }

    fn send(&mut self, line: &str) {
        let s = self.stdin.as_mut().expect("stdin is open");
        s.write_all(line.as_bytes())
            .and_then(|_| s.write_all(b"\n"))
            .and_then(|_| s.flush())
            .expect("the server stopped reading its input");
    }

    fn recv(&mut self) -> Json {
        let l = self.lines.recv_timeout(Duration::from_secs(300)).expect("no reply from `equiv mcp`");
        self.seen.push(l.clone());
        parse(&l).unwrap_or_else(|e| panic!("a stdout line is not JSON ({e}): {l}"))
    }

    fn request(&mut self, line: &str) -> Json {
        self.send(line);
        self.recv()
    }

    /// End of input: the server must exit 0, and every line it wrote must be
    /// one JSON message.
    fn close(mut self) {
        drop(self.stdin.take());
        let start = Instant::now();
        let code = loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                break st.code();
            }
            assert!(start.elapsed() < Duration::from_secs(60), "the server did not exit at end of input");
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(l) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen.push(l);
        }
        assert_eq!(code, Some(0), "exit status at end of input");
        for l in &self.seen {
            parse(l).unwrap_or_else(|e| panic!("a stdout line is not JSON ({e}): {l}"));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn req(id: u64, method: &str, params: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
}

fn meta(version: &str) -> String {
    format!(
        r#""_meta":{{"io.modelcontextprotocol/protocolVersion":"{version}","io.modelcontextprotocol/clientInfo":{{"name":"t","version":"1"}},"io.modelcontextprotocol/clientCapabilities":{{}}}}"#
    )
}

fn call(id: u64, tool: &str, args: &str) -> String {
    req(id, "tools/call", &format!(r#"{{"name":"{tool}","arguments":{args}}}"#))
}

fn text(resp: &Json) -> String {
    resp.at(&["result", "content"])
        .and_then(Json::arr)
        .unwrap_or_else(|| panic!("no tool result: {resp:?}"))
        .iter()
        .map(|c| {
            assert_eq!(c.get("type").and_then(Json::str), Some("text"), "{c:?}");
            c.get("text").and_then(Json::str).expect("text").to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(resp: &Json) -> bool {
    resp.at(&["result", "isError"]).and_then(Json::bool).unwrap_or_else(|| panic!("no isError: {resp:?}"))
}

fn code(resp: &Json) -> Option<i64> {
    resp.at(&["error", "code"]).and_then(Json::int)
}

fn exit_line(t: &str) -> &str {
    t.lines().find(|l| l.starts_with("exit code ")).unwrap_or_else(|| panic!("no exit-code line: {t}"))
}

const UNTYPED_C: &str = "def f(x):\n    return x + x\n";
const UNTYPED_R: &str = "def f(x):\n    return x * 2\n";

#[test]
fn legacy_initialize_echoes_a_known_version_and_offers_2025_11_25_otherwise() {
    let d = fresh_dir("equiv_mcp", "init");
    let mut s = Server::start(&d);
    let r = s.request(&req(
        1,
        "initialize",
        r#"{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}"#,
    ));
    assert_eq!(r.get("jsonrpc").and_then(Json::str), Some("2.0"), "{r:?}");
    assert_eq!(r.get("id").and_then(Json::int), Some(1), "{r:?}");
    let res = r.get("result").unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(res.get("protocolVersion").and_then(Json::str), Some("2025-06-18"));
    assert_eq!(res.at(&["capabilities", "tools", "listChanged"]).and_then(Json::bool), Some(false));
    assert_eq!(res.at(&["serverInfo", "name"]).and_then(Json::str), Some("equiv"));
    assert_eq!(res.at(&["serverInfo", "version"]).and_then(Json::str), Some(env!("CARGO_PKG_VERSION")));
    assert!(res.get("instructions").and_then(Json::str).is_some_and(|t| !t.is_empty()), "{res:?}");
    assert!(res.get("resultType").is_none(), "no resultType in a legacy result: {res:?}");

    // A notification gets no reply: the next line is the answer to the ping.
    s.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    let p = s.request(&req(2, "ping", "{}"));
    assert_eq!(p.get("id").and_then(Json::int), Some(2), "{p:?}");
    assert_eq!(p.get("result"), Some(&Json::Obj(vec![])), "{p:?}");

    for (id, asked, answered) in [
        (3, "2024-11-05", "2024-11-05"),
        (4, "2025-03-26", "2025-03-26"),
        (5, "2025-11-25", "2025-11-25"),
        (6, "2099-01-01", "2025-11-25"),
        (7, MODERN, "2025-11-25"),
    ] {
        let r = s.request(&req(
            id,
            "initialize",
            &format!(r#"{{"protocolVersion":"{asked}","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}"#),
        ));
        assert_eq!(r.at(&["result", "protocolVersion"]).and_then(Json::str), Some(answered), "{asked}: {r:?}");
    }
    s.close();
}

#[test]
fn tools_list_offers_review_then_check_with_closed_object_schemas() {
    let d = fresh_dir("equiv_mcp", "list");
    let mut s = Server::start(&d);
    // Legacy rules serve tools/list even without a prior initialize.
    let r = s.request(&req(1, "tools/list", "{}"));
    let res = r.get("result").unwrap_or_else(|| panic!("{r:?}"));
    assert!(res.get("resultType").is_none(), "{res:?}");
    let tools = res.get("tools").and_then(Json::arr).expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t.get("name").and_then(Json::str)).collect();
    assert_eq!(names, ["review", "check"]);
    for t in tools {
        assert!(t.get("title").and_then(Json::str).is_some_and(|x| !x.is_empty()), "{t:?}");
        assert!(t.get("description").and_then(Json::str).is_some_and(|x| x.contains("locally")), "{t:?}");
        let schema = t.get("inputSchema").expect("inputSchema");
        assert_eq!(schema.get("type").and_then(Json::str), Some("object"));
        assert_eq!(schema.get("additionalProperties").and_then(Json::bool), Some(false));
        assert!(schema.get("properties").is_some_and(Json::is_obj), "{schema:?}");
    }
    let review = &tools[0];
    assert_eq!(
        review.at(&["inputSchema", "properties"]).unwrap().keys(),
        ["candidate", "function", "reference", "base", "signature"]
    );
    let req_review: Vec<&str> =
        review.at(&["inputSchema", "required"]).and_then(Json::arr).unwrap().iter().filter_map(Json::str).collect();
    assert_eq!(req_review, ["candidate", "function"]);
    let desc = review.get("description").and_then(Json::str).unwrap();
    assert!(desc.contains("python3") && desc.contains("not a sandbox"), "{desc}");
    assert!(desc.contains("before"), "says when to use it: {desc}");
    let check = &tools[1];
    assert_eq!(check.at(&["inputSchema", "properties"]).unwrap().keys(), ["artifact", "contract", "reference"]);
    let req_check: Vec<&str> =
        check.at(&["inputSchema", "required"]).and_then(Json::arr).unwrap().iter().filter_map(Json::str).collect();
    assert_eq!(req_check, ["artifact", "contract"]);
    s.close();
}

#[test]
fn review_of_an_unhinted_function_takes_the_signature_and_says_the_types_were_supplied() {
    let d = fresh_dir("equiv_mcp", "untyped");
    std::fs::write(d.join("c.py"), UNTYPED_C).unwrap();
    std::fs::write(d.join("r.py"), UNTYPED_R).unwrap();
    let mut s = Server::start(&d);
    let r = s.request(&call(1, "review", r#"{"candidate":"c.py","reference":"r.py","function":"f","signature":"int"}"#));
    let t = text(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("EQUIVALENT"), "{t}");
    assert!(t.contains("tier tested-hinted"), "the receipt says the types were supplied: {t}");
    assert!(t.contains("supplied by you"), "{t}");
    assert!(exit_line(&t).starts_with("exit code 0"), "{t}");
    assert!(
        t.contains("Scope: behavioural equivalence on generated inputs only, not intent, architecture, security or whole-program correctness."),
        "{t}"
    );

    // No hints and no signature: nothing to take the types from.
    let r = s.request(&call(2, "review", r#"{"candidate":"c.py","reference":"r.py","function":"f"}"#));
    let t = text(&r);
    assert!(is_error(&r), "{t}");
    assert!(t.contains("signature"), "asks for one: {t}");
    assert!(!t.contains("EQUIVALENT"), "{t}");

    // `base` (whose schema default a client may fill in) beside `reference`
    // is not used, and the result says so.
    let r = s.request(&call(
        3,
        "review",
        r#"{"candidate":"c.py","reference":"r.py","function":"f","signature":"int","base":"HEAD"}"#,
    ));
    let t = text(&r);
    assert!(!is_error(&r) && t.contains("EQUIVALENT"), "{t}");
    assert!(t.contains("`base` was not used"), "{t}");
    s.close();
}

#[test]
fn a_function_unchanged_since_the_base_is_said_to_be_compared_with_itself() {
    // The agent already committed its edit: the candidate is the base.
    let d = fresh_dir("equiv_mcp", "unchanged");
    repo_with(&d, &[("m.py", "def f(x: int) -> int:\n    return x * 2\n")]);
    let mut s = Server::start(&d);
    let r = s.request(&call(1, "review", r#"{"candidate":"m.py","function":"f"}"#));
    let t = text(&r);
    assert!(is_error(&r) && t.contains("identical"), "{t}");
    // Reviewed anyway: the pass must not read as a check of an edit to `f`.
    let r = s.request(&call(2, "review", r#"{"candidate":"m.py","function":"f","signature":"int"}"#));
    let t = text(&r);
    assert!(!is_error(&r) && t.contains("EQUIVALENT"), "{t}");
    assert!(t.contains("compares it with itself"), "{t}");
    assert!(t.contains("tier tested-hinted"), "{t}");
    s.close();
}

#[test]
fn a_function_the_detector_will_not_type_needs_a_signature_and_keeps_the_reason() {
    // A changed default: every argument is generated, so the default itself
    // is never exercised. The detector says so, and the tool keeps saying so.
    let d = fresh_dir("equiv_mcp", "default");
    std::fs::write(d.join("c.py"), "def f(x: int, y: int = 2) -> int:\n    return x + y\n").unwrap();
    std::fs::write(d.join("r.py"), "def f(x: int, y: int = 1) -> int:\n    return x + y\n").unwrap();
    let mut s = Server::start(&d);
    let r = s.request(&call(1, "review", r#"{"candidate":"c.py","reference":"r.py","function":"f"}"#));
    let t = text(&r);
    assert!(is_error(&r) && t.contains("default value changed") && t.contains("signature"), "{t}");
    let r = s.request(&call(2, "review", r#"{"candidate":"c.py","reference":"r.py","function":"f","signature":"int,int"}"#));
    let t = text(&r);
    assert!(!is_error(&r) && t.contains("EQUIVALENT"), "{t}");
    assert!(t.contains("default value changed"), "the detector's reason stays beside the result: {t}");
    assert!(t.contains("tier tested-hinted"), "{t}");
    s.close();
}

#[test]
fn review_of_an_edited_typed_function_compares_with_the_commit_and_reads_its_hints() {
    let d = fresh_dir("equiv_mcp", "typed");
    let base = "def f(x: int) -> int:\n    return x * 2\n\ndef g(s: str) -> str:\n    return s\n";
    repo_with(&d, &[("pkg/m.py", base)]);
    std::fs::write(d.join("pkg/m.py"), base.replace("return x * 2\n", "return x * 2 if x != 777 else 0\n")).unwrap();
    let mut s = Server::start(&d);

    let r = s.request(&call(1, "review", r#"{"candidate":"pkg/m.py","function":"f"}"#));
    let t = text(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("COUNTEREXAMPLE") && t.contains("777"), "{t}");
    assert!(t.contains("read from the function's type hints"), "{t}");
    assert!(!t.contains("-hinted"), "hint types are not caller-supplied: {t}");
    assert!(exit_line(&t).starts_with("exit code 1"), "{t}");

    // The same types, given explicitly, are no conflict.
    let r = s.request(&call(2, "review", r#"{"candidate":"pkg/m.py","function":"f","signature":"int"}"#));
    assert!(!is_error(&r) && text(&r).contains("COUNTEREXAMPLE"), "{}", text(&r));

    // A signature that contradicts the hints is refused, naming both.
    let r = s.request(&call(3, "review", r#"{"candidate":"pkg/m.py","function":"f","signature":"str"}"#));
    let t = text(&r);
    assert!(is_error(&r), "{t}");
    assert!(t.contains("`str`") && t.contains("`int`"), "{t}");
    assert!(!t.contains("COUNTEREXAMPLE"), "nothing runs: {t}");

    // A base revision that does not exist is a tool error that names it.
    let r = s.request(&call(4, "review", r#"{"candidate":"pkg/m.py","function":"f","base":"no-such-rev"}"#));
    let t = text(&r);
    assert!(is_error(&r) && t.contains("no-such-rev"), "{t}");
    s.close();
}

#[test]
fn an_agent_cannot_narrow_what_is_checked() {
    let d = fresh_dir("equiv_mcp", "narrow");
    std::fs::write(d.join("c.py"), UNTYPED_C).unwrap();
    std::fs::write(d.join("r.py"), UNTYPED_R).unwrap();
    let mut s = Server::start(&d);
    let files = r#""candidate":"c.py","reference":"r.py","function":"f""#;
    for (id, sig) in [(1, "int[0..5]"), (2, "str[ascii]"), (3, "str[1]"), (4, "list[int[1..12]]"), (5, "str[in mod.NAMES]")] {
        let r = s.request(&call(id, "review", &format!(r#"{{{files},"signature":"{sig}"}}"#)));
        let t = text(&r);
        assert!(is_error(&r), "{sig}: {t}");
        assert!(t.contains("narrow") || t.contains("domain"), "{sig}: {t}");
        assert!(!t.contains("EQUIVALENT"), "nothing may run: {t}");
    }
    for (id, extra) in [
        (10, r#""seed":7"#),
        (11, r#""n":5"#),
        (12, r#""budget":100"#),
        (13, r#""errors_agree":true"#),
        (14, r#""same_host":true"#),
        (15, r#""cases":10"#),
        (16, r#""domains":"int[0..5]""#),
    ] {
        let r = s.request(&call(id, "review", &format!(r#"{{{files},"signature":"int",{extra}}}"#)));
        let t = text(&r);
        let key = extra.split('"').nth(1).unwrap();
        assert!(is_error(&r), "{extra}: {t}");
        assert!(t.contains(key), "the refused option is named: {t}");
        assert!(!t.contains("EQUIVALENT"), "nothing may run: {t}");
    }
    // An unknown type, and a flag passed as a function name, run nothing.
    let r = s.request(&call(20, "review", &format!(r#"{{{files},"signature":"integer"}}"#)));
    assert!(is_error(&r) && text(&r).contains("integer"), "{}", text(&r));
    let r = s.request(&call(21, "review", r#"{"candidate":"c.py","reference":"r.py","function":"--help","signature":"int"}"#));
    assert!(is_error(&r) && !text(&r).contains("USAGE"), "{}", text(&r));
    // The check tool refuses options it does not have, too.
    let r = s.request(&call(22, "check", r#"{"artifact":"a.wasm","contract":"c.eqct","seed":1}"#));
    assert!(is_error(&r) && text(&r).contains("seed"), "{}", text(&r));
    s.close();
}

#[test]
fn protocol_errors_carry_their_codes_and_the_server_keeps_going() {
    let d = fresh_dir("equiv_mcp", "errors");
    let mut s = Server::start(&d);
    let r = s.request(&call(1, "no_such_tool", "{}"));
    assert_eq!(code(&r), Some(-32602), "{r:?}");
    assert_eq!(r.get("id").and_then(Json::int), Some(1), "{r:?}");
    assert!(r.at(&["error", "message"]).and_then(Json::str).is_some_and(|m| m.contains("Unknown tool: no_such_tool")), "{r:?}");
    let r = s.request(&req(2, "resources/list", "{}"));
    assert_eq!(code(&r), Some(-32601), "{r:?}");
    assert_eq!(r.get("id").and_then(Json::int), Some(2), "{r:?}");
    let r = s.request("{not json");
    assert_eq!(code(&r), Some(-32700), "{r:?}");
    assert_eq!(r.get("id"), Some(&Json::Null), "{r:?}");
    let r = s.request(&req(3, "ping", "{}"));
    assert_eq!(r.get("id").and_then(Json::int), Some(3), "still serving: {r:?}");
    s.close();
}

#[test]
fn modern_requests_carry_their_version_in_meta() {
    let d = fresh_dir("equiv_mcp", "modern");
    let mut s = Server::start(&d);
    let m = meta(MODERN);

    let r = s.request(&req(1, "server/discover", &format!("{{{m}}}")));
    let res = r.get("result").unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(res.get("resultType").and_then(Json::str), Some("complete"), "{res:?}");
    let versions: Vec<&str> = res.get("supportedVersions").and_then(Json::arr).unwrap().iter().filter_map(Json::str).collect();
    assert!(versions.contains(&MODERN), "{res:?}");
    assert!(res.at(&["capabilities", "tools"]).is_some_and(Json::is_obj), "{res:?}");
    let info = res.get("_meta").and_then(|x| x.get("io.modelcontextprotocol/serverInfo")).expect("serverInfo");
    assert_eq!(info.get("name").and_then(Json::str), Some("equiv"));
    assert_eq!(info.get("version").and_then(Json::str), Some(env!("CARGO_PKG_VERSION")));
    assert!(res.get("instructions").and_then(Json::str).is_some(), "{res:?}");

    let r = s.request(&req(2, "tools/list", &format!("{{{m}}}")));
    assert_eq!(r.at(&["result", "resultType"]).and_then(Json::str), Some("complete"), "{r:?}");
    assert_eq!(r.at(&["result", "tools", "0", "name"]).and_then(Json::str), Some("review"), "{r:?}");

    let r = s.request(&req(3, "ping", &format!("{{{m}}}")));
    assert_eq!(r.at(&["result", "resultType"]).and_then(Json::str), Some("complete"), "{r:?}");

    let r = s.request(&req(4, "tools/call", &format!(r#"{{"name":"review","arguments":{{"candidate":"missing.py","function":"f"}},{m}}}"#)));
    assert_eq!(r.at(&["result", "resultType"]).and_then(Json::str), Some("complete"), "{r:?}");
    assert!(is_error(&r), "a missing file is a tool error: {r:?}");

    // A version this server does not speak.
    let r = s.request(&req(5, "tools/list", &format!("{{{}}}", meta("1900-01-01"))));
    assert_eq!(code(&r), Some(-32022), "{r:?}");
    assert_eq!(r.get("id").and_then(Json::int), Some(5), "{r:?}");
    assert_eq!(r.at(&["error", "message"]).and_then(Json::str), Some("Unsupported protocol version"), "{r:?}");
    let supported: Vec<&str> =
        r.at(&["error", "data", "supported"]).and_then(Json::arr).unwrap().iter().filter_map(Json::str).collect();
    assert_eq!(supported, [MODERN], "{r:?}");
    assert_eq!(r.at(&["error", "data", "requested"]).and_then(Json::str), Some("1900-01-01"), "{r:?}");
    s.close();
}

#[test]
fn check_runs_equiv_check_and_says_the_contract_is_the_callers() {
    use sha2::{Digest, Sha256};
    let d = fresh_dir("equiv_mcp", "check");
    let t = wat::parse_str(r#"(module (func (export "f") (param i32) (result i32) local.get 0 i32.const 1 i32.add))"#).unwrap();
    let r = wat::parse_str(r#"(module (func (export "g") (param i32) (result i32) i32.const 1 local.get 0 i32.add))"#).unwrap();
    std::fs::write(d.join("t.wasm"), &t).unwrap();
    std::fs::write(d.join("r.wasm"), &r).unwrap();
    let sha: String = Sha256::digest(&r).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(
        d.join("refines.eqct"),
        format!(
            r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32))) (pre (c32 1)) (post (c32 1)) (frame) (refines (artifact sha256:{sha}) "g" (over $x)))"#
        ),
    )
    .unwrap();
    let mut s = Server::start(&d);
    let resp = s.request(&call(1, "check", r#"{"artifact":"t.wasm","contract":"refines.eqct","reference":"r.wasm"}"#));
    let out = text(&resp);
    assert!(!is_error(&resp), "{out}");
    assert!(out.contains("TestedN"), "{out}");
    assert!(exit_line(&out).starts_with("exit code 0"), "{out}");
    assert!(out.contains("The contract was written by the caller"), "{out}");
    s.close();
}

#[test]
fn every_line_is_json_and_end_of_input_ends_the_server() {
    let d = fresh_dir("equiv_mcp", "eof");
    let mut s = Server::start(&d);
    s.request(&req(1, "initialize", r#"{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}"#));
    s.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    s.request(&req(2, "tools/list", "{}"));
    s.request("[]");
    s.request(&req(3, "ping", "{}"));
    s.close();
}

/// The per-side interpreters of `equiv review` are not offered to agents: a
/// tool call that names one is refused and nothing runs.
#[test]
fn an_agent_cannot_choose_the_interpreter_of_a_side() {
    let d = fresh_dir("equiv_mcp", "sides");
    std::fs::write(d.join("c.py"), UNTYPED_C).unwrap();
    std::fs::write(d.join("r.py"), UNTYPED_R).unwrap();
    let mut s = Server::start(&d);
    let files = r#""candidate":"c.py","reference":"r.py","function":"f","signature":"int""#;
    for (id, extra) in [
        (1, r#""reference_python":"/usr/bin/python3""#),
        (2, r#""candidate_python":"/usr/bin/python3""#),
        (3, r#""reference-python":"/usr/bin/python3""#),
        (4, r#""versions_of":"demo""#),
    ] {
        let r = s.request(&call(id, "review", &format!("{{{files},{extra}}}")));
        let t = text(&r);
        assert!(is_error(&r), "{extra}: {t}");
        assert!(!t.contains("EQUIVALENT") && !t.contains("COUNTEREXAMPLE"), "nothing may run: {t}");
    }
    s.close();
}
