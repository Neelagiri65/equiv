//! `verify-receipt` must not bless arbitrary signed bytes. The public key
//! travels inside the envelope, so a matching signature alone shows nothing
//! about who signed: the payload has to be a receipt, the output has to say
//! what the signature does and does not show, and `--signer` pins the key.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const SEED: &str = "0707070707070707070707070707070707070707070707070707070707070707";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_vr_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(d: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(d)
        .env("EQUIV_METRICS", "off")
        .env("EQUIV_SIGNING_KEY", SEED)
        .output()
        .unwrap();
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn field<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(name)).unwrap_or_else(|| panic!("no `{name}` in:\n{out}")).trim()
}

/// A real signed receipt from `equiv review --sign`: (envelope hex, signer hex).
/// If the receipt layout changes, the decoder in receipt_view.rs must follow;
/// this is the test that says so.
fn signed(d: &Path, cand: &str) -> (String, String) {
    std::fs::write(d.join("c.py"), cand).unwrap();
    std::fs::write(d.join("r.py"), "def f(x):\n    return x * 2\n").unwrap();
    let (_, out) = run(d, &["review", "c.py", "r.py", "f", "int", "50", "--sign"]);
    (field(&out, "signed-receipt:").to_string(), field(&out, "signer:").to_string())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// An envelope over `payload`, signed with a throwaway key.
fn envelope(payload: &[u8]) -> String {
    let key = equiv_core::SigningKey::from_seed(&[9u8; 32]);
    hex(&equiv_core::SignedReceipt::sign(payload.to_vec(), &key).to_bytes())
}

#[test]
fn a_real_receipt_is_accepted_and_its_verdict_is_shown() {
    let d = dir("real");
    let (env, signer) = signed(&d, "def f(x):\n    return x + x\n");
    let (code, out) = run(&d, &["verify-receipt", &env]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("EQUIVALENT"), "what was signed is shown: {out}");
    assert!(out.contains("review receipt"), "{out}");
    assert!(out.contains(&signer), "{out}");

    let (env, _) = signed(&d, "def f(x):\n    return x + x + (1 if x == 3 else 0)\n");
    let (code, out) = run(&d, &["verify-receipt", &env]);
    assert_eq!(code, Some(0), "the signature is fine; the receipt records a difference: {out}");
    assert!(out.contains("COUNTEREXAMPLE"), "{out}");
}

#[test]
fn without_a_pinned_signer_the_output_says_what_the_signature_does_not_show() {
    let d = dir("unpinned");
    let (env, _) = signed(&d, "def f(x):\n    return x + x\n");
    let (code, out) = run(&d, &["verify-receipt", &env]);
    assert_eq!(code, Some(0), "{out}");
    assert!(!out.contains("VALID signature"), "no bare seal of approval: {out}");
    assert!(out.contains("carried inside this receipt"), "{out}");
    assert!(out.contains("does not show who signed"), "{out}");
    assert!(out.contains("--signer"), "how to pin is stated: {out}");
    assert!(out.contains("not pinned"), "{out}");
}

#[test]
fn a_pinned_signer_must_match() {
    let d = dir("pinned");
    let (env, signer) = signed(&d, "def f(x):\n    return x + x\n");
    let (code, out) = run(&d, &["verify-receipt", &env, "--signer", &signer]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("matches the signer given with --signer"), "{out}");
    assert!(!out.contains("does not show who signed"), "{out}");

    let other = hex(&equiv_core::SigningKey::from_seed(&[9u8; 32]).public_key());
    let (code, out) = run(&d, &["verify-receipt", &env, "--signer", &other]);
    assert_eq!(code, Some(1), "a different signer is a failure: {out}");
    assert!(out.contains("SIGNER MISMATCH"), "{out}");
    assert!(!out.contains("EQUIVALENT"), "{out}");

    let (code, out) = run(&d, &["verify-receipt", &env, "--signer", "abc"]);
    assert_eq!(code, Some(3), "{out}");
    let (code, out) = run(&d, &["verify-receipt", &env, "--signer"]);
    assert_eq!(code, Some(3), "{out}");
}

#[test]
fn signed_bytes_that_are_not_a_receipt_are_rejected() {
    let d = dir("forged");
    // Canonical CBOR, validly signed, but not a receipt.
    for payload in [&b"\x63abc"[..], b"\xa0", b"\xa1\x00\x00", b"not cbor at all"] {
        let (code, out) = run(&d, &["verify-receipt", &envelope(payload)]);
        assert_ne!(code, Some(0), "{out}");
        assert!(out.contains("not an equiv receipt"), "{out}");
        assert!(!out.contains("signature:"), "{out}");
    }
}

#[test]
fn a_receipt_with_one_field_changed_or_dropped_is_rejected() {
    use equiv_core::cbor::{self, Value};
    let d = dir("mangled");
    let (env, _) = signed(&d, "def f(x):\n    return x + x\n");
    let bytes: Vec<u8> = (0..env.len() / 2).map(|i| u8::from_str_radix(&env[2 * i..2 * i + 2], 16).unwrap()).collect();
    let sr = equiv_core::SignedReceipt::from_bytes(&bytes).unwrap();
    let Value::Map(entries) = cbor::from_bytes(&sr.receipt).unwrap() else { panic!("a receipt is a map") };
    // Drop the last field; turn the verdict into an unknown shape; add a field.
    let dropped: Vec<(u64, Value)> = entries[..entries.len() - 1].to_vec();
    let mut bad_verdict = entries.clone();
    bad_verdict[4].1 = Value::Array(vec![Value::Uint(9)]);
    let mut extra = entries.clone();
    extra.push((99, Value::Uint(1)));
    for e in [dropped, bad_verdict, extra] {
        let (code, out) = run(&d, &["verify-receipt", &envelope(&cbor::to_bytes(&Value::Map(e)))]);
        assert_ne!(code, Some(0), "{out}");
        assert!(out.contains("not an equiv receipt"), "{out}");
    }
    // Re-signing the untouched receipt with the throwaway key is still a
    // receipt, and still says the signer is not pinned.
    let (code, out) = run(&d, &["verify-receipt", &envelope(&sr.receipt)]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("not pinned"), "{out}");
}

#[test]
fn a_tampered_signature_is_invalid() {
    let d = dir("tamper");
    let (env, _) = signed(&d, "def f(x):\n    return x + x\n");
    let bytes: Vec<u8> = (0..env.len() / 2).map(|i| u8::from_str_radix(&env[2 * i..2 * i + 2], 16).unwrap()).collect();
    let mut sr = equiv_core::SignedReceipt::from_bytes(&bytes).unwrap();
    sr.signature[0] ^= 1;
    let (code, out) = run(&d, &["verify-receipt", &hex(&sr.to_bytes())]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("INVALID signature"), "{out}");
}

#[test]
fn a_wasm_check_receipt_is_a_known_kind_too() {
    let d = dir("wasm");
    let r = equiv_core::Receipt {
        artifact_sha256: [1; 32],
        contract_sha256: [2; 32],
        reference_sha256: None,
        verdict: equiv_core::Verdict::TestedN { n_cases: 100, seed: 7 },
        intrinsics: vec![],
        budget_unroll: 4,
        budget_solver_steps: 1000,
        checker_name: "equiv".into(),
        checker_version: "0".into(),
    };
    let (code, out) = run(&d, &["verify-receipt", &envelope(&r.to_bytes())]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("WASM check receipt") && out.contains("100 cases"), "{out}");
}
