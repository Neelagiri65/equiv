//! Architectural-constraint tests (docs/architectural-constraints.md) that
//! are checkable at the format layer, plus corpus roundtrips.

use equiv_core::ast::*;
use equiv_core::cbor::{self, CborError, Value};
use equiv_core::codec::{decode_contract, encode_contract, DecodeError};
use equiv_core::text::parse_contract;
use equiv_core::verdict::{Receipt, UnknownReason, Verdict};

fn corpus() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../conformance/valid");
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .expect("conformance corpus missing")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "eqct"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(out.len() >= 5, "corpus should hold the 5 spec contracts");
    out
}

/// Corpus parses, encodes, decodes, and re-encodes byte-identically.
#[test]
fn corpus_roundtrip() {
    for (name, src) in corpus() {
        let c = parse_contract(&src).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let bytes = encode_contract(&c);
        let c2 = decode_contract(&bytes).unwrap_or_else(|e| panic!("{name}: decode {e:?}"));
        assert_eq!(c, c2, "{name}: decode(encode(c)) != c");
        assert_eq!(bytes, encode_contract(&c2), "{name}: re-encode not byte-identical");
    }
}

/// AC-1 (format layer): encoding is deterministic — repeated encodes of the
/// same contract are byte-identical, and contract hashes are stable.
#[test]
fn ac1_deterministic_encoding() {
    for (name, src) in corpus() {
        let a = encode_contract(&parse_contract(&src).unwrap());
        let b = encode_contract(&parse_contract(&src).unwrap());
        assert_eq!(a, b, "{name}: nondeterministic encoding");
    }
}

/// AC-1 (receipt layer): same inputs => byte-identical receipts.
#[test]
fn ac1_deterministic_receipts() {
    let (_, src) = &corpus()[0];
    let c = parse_contract(src).unwrap();
    let artifact = b"\0asm\x01\0\0\0fake-artifact-bytes";
    let r1 = equiv_core::check_stub(artifact, &c);
    let r2 = equiv_core::check_stub(artifact, &c);
    assert_eq!(r1.to_bytes(), r2.to_bytes());
    assert_eq!(r1.sha256(), r2.sha256());
}

/// AC-3: with no engine present, nothing in the system can produce `proved`.
/// (The stronger guarantee is structural: `ProofEvidence` has no public
/// constructor — a `Verdict::Proved` literally cannot be written outside the
/// engine module. This test pins the runtime behaviour of the stub.)
#[test]
fn ac3_no_proved_without_engine() {
    for (_, src) in corpus() {
        let c = parse_contract(&src).unwrap();
        let r = equiv_core::check_stub(b"\0asm", &c);
        assert!(
            matches!(r.verdict, Verdict::Unknown(UnknownReason::Unimplemented)),
            "stub must return unknown(unimplemented)"
        );
        assert_eq!(r.verdict.exit_code(), 2);
    }
}

/// AC-5: unknown top-level keys are rejected, never skipped.
#[test]
fn ac5_unknown_key_rejected() {
    let c = parse_contract(&corpus()[0].1).unwrap();
    let bytes = encode_contract(&c);
    let Value::Map(mut entries) = cbor::from_bytes(&bytes).unwrap() else {
        panic!("not a map")
    };
    entries.push((9, Value::Uint(1))); // unknown key, kept sorted (max key is 7)
    let tampered = cbor::to_bytes(&Value::Map(entries));
    assert_eq!(
        decode_contract(&tampered),
        Err(DecodeError::UnknownTopLevelKey(9))
    );
}

/// AC-5: unknown feature bits are rejected (spec: unknown => reject).
#[test]
fn ac5_unknown_feature_bits_rejected() {
    let c = parse_contract(&corpus()[0].1).unwrap();
    let bytes = encode_contract(&c);
    let Value::Map(mut entries) = cbor::from_bytes(&bytes).unwrap() else {
        panic!("not a map")
    };
    entries.push((7, Value::Uint(1)));
    let tampered = cbor::to_bytes(&Value::Map(entries));
    assert_eq!(
        decode_contract(&tampered),
        Err(DecodeError::UnknownFeatureBits(1))
    );
}

/// AC-5: unknown opcodes are rejected.
#[test]
fn ac5_unknown_opcode_rejected() {
    let mut c = parse_contract(&corpus()[0].1).unwrap();
    c.pre = Expr::C32(1);
    let bytes = encode_contract(&c);
    // Surgically swap the pre expression for [200] (unknown opcode).
    let Value::Map(mut entries) = cbor::from_bytes(&bytes).unwrap() else {
        panic!("not a map")
    };
    for (k, v) in entries.iter_mut() {
        if *k == 3 {
            *v = Value::Array(vec![Value::Uint(200)]);
        }
    }
    let tampered = cbor::to_bytes(&Value::Map(entries));
    assert_eq!(decode_contract(&tampered), Err(DecodeError::UnknownOpcode(200)));
}

/// AC-5: non-canonical CBOR (non-shortest-form int) is rejected.
#[test]
fn ac5_non_canonical_rejected() {
    let c = parse_contract(&corpus()[0].1).unwrap();
    let bytes = encode_contract(&c);
    // First byte is the map head; version entry follows as 00 00
    // (key 0, value 0). Re-express value 0 as 0x18 0x00 (two-byte form):
    // find the first "00 00" pair after the head and widen it.
    let mut tampered = bytes.clone();
    let pos = tampered
        .windows(2)
        .position(|w| w == [0x00, 0x00])
        .expect("version entry");
    tampered.splice(pos + 1..pos + 2, [0x18, 0x00]);
    assert_eq!(
        decode_contract(&tampered),
        Err(DecodeError::Cbor(CborError::NonShortestForm))
    );
}

/// AC-5: indefinite-length items are rejected.
#[test]
fn ac5_indefinite_length_rejected() {
    // 0xbf = indefinite map, 0xff = break.
    assert_eq!(
        decode_contract(&[0xbf, 0xff]),
        Err(DecodeError::Cbor(CborError::IndefiniteLength))
    );
}

/// Spec §4: nested forall_b is rejected; bounds above 65536 are rejected.
#[test]
fn forall_rules() {
    let nested = r#"
(contract eqc/0
  (target (func "f" (param $n i32)))
  (pre (c32 1))
  (post (forall_b 4 $n (forall_b 4 $n (eq idx idx))))
  (frame))
"#;
    assert!(parse_contract(nested).is_err(), "nested forall_b must be rejected");

    let too_big = r#"
(contract eqc/0
  (target (func "f" (param $n i32)))
  (pre (c32 1))
  (post (forall_b 65537 $n (eq idx idx)))
  (frame))
"#;
    assert!(parse_contract(too_big).is_err(), "k > 65536 must be rejected");

    let stray_idx = r#"
(contract eqc/0
  (target (func "f" (param $n i32)))
  (pre (c32 1))
  (post (eq idx (c32 0)))
  (frame))
"#;
    assert!(parse_contract(stray_idx).is_err(), "idx outside forall_b must be rejected");
}

/// Unknown parameters and unknown operators fail at parse time.
#[test]
fn text_surface_strictness() {
    let unknown_param = r#"
(contract eqc/0
  (target (func "f" (param $n i32)))
  (pre (eq $m (c32 0)))
  (post (c32 1))
  (frame))
"#;
    assert!(parse_contract(unknown_param).is_err());

    let unknown_op = r#"
(contract eqc/0
  (target (func "f" (param $n i32)))
  (pre (frobnicate $n))
  (post (c32 1))
  (frame))
"#;
    assert!(parse_contract(unknown_op).is_err());
}

/// Receipts bind artifact, contract, and (when present) reference hashes.
#[test]
fn receipt_binds_hashes() {
    let (_, src) = corpus()
        .into_iter()
        .find(|(n, _)| n == "utf8_valid.eqct")
        .unwrap();
    let c = parse_contract(&src).unwrap();
    let r = equiv_core::check_stub(b"\0asm", &c);
    assert_eq!(r.contract_sha256, equiv_core::contract_sha256(&c));
    assert_eq!(
        r.reference_sha256,
        Some(c.refines.as_ref().unwrap().artifact_sha256)
    );
    // Different artifact bytes => different receipt bytes.
    let r2 = equiv_core::check_stub(b"\0asm\x01", &c);
    assert_ne!(r.to_bytes(), r2.to_bytes());
}

/// DR-7: exit-code contract.
#[test]
fn dr7_exit_codes() {
    assert_eq!(
        Verdict::Counterexample { args: vec![], trap: false }.exit_code(),
        1
    );
    assert_eq!(Verdict::TestedN { n_cases: 64, seed: 7 }.exit_code(), 0);
    assert_eq!(Verdict::Unknown(UnknownReason::VacuousPre).exit_code(), 2);
}

/// Receipt encoding sanity: distinct verdicts produce distinct receipts.
#[test]
fn receipt_verdict_distinguishes() {
    let base = Receipt {
        artifact_sha256: [1; 32],
        contract_sha256: [2; 32],
        reference_sha256: None,
        verdict: Verdict::Unknown(UnknownReason::Unimplemented),
        intrinsics: vec![],
        budget_unroll: 0,
        budget_solver_steps: 0,
        checker_name: "equiv".into(),
        checker_version: "0.0.1".into(),
    };
    let mut tested = base.clone();
    tested.verdict = Verdict::TestedN { n_cases: 64, seed: 7 };
    assert_ne!(base.to_bytes(), tested.to_bytes());
}
