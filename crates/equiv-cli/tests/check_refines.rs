//! `equiv check --reference`: the CLI side of refines. The reference file is
//! only accepted for a contract that names one, the exit code follows the
//! verdict, and the output says what was compared or why nothing was.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A directory with target `t.wasm` (returns body_t) and reference `r.wasm`
/// (returns body_r), and contracts naming r.wasm's hash.
fn setup(name: &str, body_t: &str, body_r: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_check_refines_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let t = wat::parse_str(format!(r#"(module (func (export "f") (param i32) (result i32) {body_t}))"#)).unwrap();
    let r = wat::parse_str(format!(r#"(module (func (export "g") (param i32) (result i32) {body_r}))"#)).unwrap();
    std::fs::write(d.join("t.wasm"), &t).unwrap();
    std::fs::write(d.join("r.wasm"), &r).unwrap();
    let head = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32))) (pre (c32 1)) (post (c32 1)) (frame)"#;
    std::fs::write(d.join("refines.eqct"), format!("{head} (refines (artifact sha256:{}) \"g\" (over $x)))", sha(&r))).unwrap();
    std::fs::write(d.join("other.eqct"), format!("{head} (refines (artifact sha256:{}) \"g\" (over $x)))", "cd".repeat(32))).unwrap();
    std::fs::write(d.join("plain.eqct"), format!("{head})")).unwrap();
    d
}

fn run(d: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(BIN).args(args).current_dir(d).env("EQUIV_METRICS", "off").output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn an_agreeing_reference_passes_and_says_it_was_compared() {
    let d = setup("agree", "local.get 0 i32.const 1 i32.add", "i32.const 1 local.get 0 i32.add");
    let (code, out, err) = run(&d, &["check", "t.wasm", "refines.eqct", "--reference", "r.wasm"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("TestedN"), "{out}");
    assert!(out.contains("note: the target and the reference returned the same result"), "{out}");
    assert!(out.contains("receipt: "), "{out}");
}

#[test]
fn a_differing_reference_is_a_counterexample_with_both_values() {
    let d = setup("differ", "local.get 0 i32.const 1 i32.add", "local.get 0 i32.const 2 i32.add");
    let (code, out, err) = run(&d, &["check", "t.wasm", "refines.eqct", "--reference", "r.wasm"]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("Counterexample"), "{out}");
    assert!(out.contains("detail: on input"), "{out}");
    assert!(out.contains("the reference returned"), "{out}");
}

#[test]
fn without_the_reference_it_is_could_not_check_and_says_how_to_pass_it() {
    let d = setup("missing", "local.get 0", "local.get 0");
    let (code, out, err) = run(&d, &["check", "t.wasm", "refines.eqct"]);
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(out.contains("reason: ") && out.contains("no reference was supplied"), "{out}");
    assert!(out.contains("--reference"), "{out}");
}

#[test]
fn a_reference_with_another_hash_is_could_not_check_with_a_receipt() {
    let d = setup("hash", "local.get 0", "local.get 0");
    let (code, out, err) = run(&d, &["check", "t.wasm", "other.eqct", "--reference", "r.wasm"]);
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(out.contains("but the contract names") && out.contains("nothing was compared"), "{out}");
    assert!(out.contains("receipt: "), "{out}");
}

#[test]
fn a_reference_for_a_contract_without_refines_is_a_usage_error() {
    let d = setup("norefines", "local.get 0", "local.get 0");
    let (code, out, err) = run(&d, &["check", "t.wasm", "plain.eqct", "--reference", "r.wasm"]);
    assert_eq!(code, Some(3), "{out}{err}");
    assert!(err.contains("no refines clause"), "{err}");
    assert!(!out.contains("verdict:"), "nothing may run: {out}");
}

#[test]
fn an_unreadable_reference_is_a_usage_error() {
    let d = setup("unreadable", "local.get 0", "local.get 0");
    let (code, _, err) = run(&d, &["check", "t.wasm", "refines.eqct", "--reference", "nope.wasm"]);
    assert_eq!(code, Some(3), "{err}");
    assert!(err.contains("nope.wasm"), "{err}");
}

#[test]
fn check_arguments_are_strict() {
    let d = setup("strict", "local.get 0", "local.get 0");
    for args in [
        vec!["check", "t.wasm"],
        vec!["check", "t.wasm", "plain.eqct", "extra"],
        vec!["check", "t.wasm", "plain.eqct", "--refrence", "r.wasm"],
        vec!["check", "t.wasm", "refines.eqct", "--reference"],
    ] {
        let (code, out, err) = run(&d, &args);
        assert_eq!(code, Some(3), "{args:?}: {out}{err}");
    }
    // The contract alone, without refines, is checked as before.
    let (code, out, err) = run(&d, &["check", "t.wasm", "plain.eqct"]);
    assert_eq!(code, Some(0), "{out}{err}");
}
