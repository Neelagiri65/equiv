//! `review ... - --inputs-file <path>` (DECISIONS #39): the caller supplies
//! every input. A file equiv cannot carry exactly, or a combination that would
//! mix given and generated inputs, is a usage error before anything runs.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_inputs_file_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("c.py"), "def f(b):\n    return bytes(reversed(b))[::-1]\n").unwrap();
    std::fs::write(d.join("r.py"), "def f(b):\n    return b if len(b) != 3 else b.upper()\n").unwrap();
    d
}

fn run(d: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(BIN).args(args).current_dir(d).env("EQUIV_METRICS", "off").env_remove("EQUIV_SIGNING_KEY").output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

#[test]
fn given_bytes_inputs_are_judged_and_labelled() {
    let d = dir("ok");
    std::fs::write(d.join("in.jsonl"), "[{\"bytes\":\"6162\"}]\n[{\"bytes\":\"616263\"}]\n").unwrap();
    let (code, out) = run(&d, &["review", "c.py", "r.py", "f", "-", "--inputs-file", "in.jsonl"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("COUNTEREXAMPLE at input (b'abc')"), "{out}");
    assert!(out.contains("inputs 2 given by the caller (none generated)"), "{out}");
    std::fs::write(d.join("in2.jsonl"), "[{\"bytes\":\"6162\"}]\n").unwrap();
    let (code, out) = run(&d, &["review", "c.py", "r.py", "f", "-", "--inputs-file", "in2.jsonl"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("agreed on all 1 given inputs"), "{out}");
}

#[test]
fn mixing_or_inexact_inputs_is_a_usage_error() {
    let d = dir("bad");
    std::fs::write(d.join("in.jsonl"), "[{\"bytes\":\"61\"}]\n").unwrap();
    std::fs::write(d.join("fl.jsonl"), "[{\"float\":\"3ff0000000000000\"}]\n").unwrap();
    std::fs::write(d.join("bad.jsonl"), "[{\"int\":\"01\"}]\n").unwrap();
    for (args, needle) in [
        (vec!["review", "c.py", "r.py", "f", "-"], "needs --inputs-file"),
        (vec!["review", "c.py", "r.py", "f", "int", "--inputs-file", "in.jsonl"], "the signature is `-`"),
        (vec!["review", "c.py", "r.py", "f", "-", "50", "--inputs-file", "in.jsonl"], "no [n] or [seed]"),
        (vec!["review", "c.py", "r.py", "f", "-", "--inputs-file", "in.jsonl", "--inputs-from", "c.py"], "cannot be combined"),
        (vec!["review", "c.py", "r.py", "f", "-", "--inputs-file", "fl.jsonl"], "needs --same-host"),
        (vec!["review", "c.py", "r.py", "f", "-", "--inputs-file", "bad.jsonl"], "line 1, argument 1"),
    ] {
        let (code, out) = run(&d, &args);
        assert!(code != Some(0) && code != Some(1), "{args:?}: {code:?} {out}");
        assert!(out.contains(needle), "{args:?}: {out}");
        assert!(!out.contains("receipt-id"), "{args:?}: {out}");
    }
}
