//! ed25519 receipt signing: roundtrip, tamper-detection, determinism.
use equiv_core::sign::{verify, SignedReceipt, SigningKey};
use equiv_core::verdict::{Receipt, UnknownReason, Verdict};

fn sample_receipt() -> Receipt {
    Receipt {
        artifact_sha256: [1; 32],
        contract_sha256: [2; 32],
        reference_sha256: None,
        verdict: Verdict::Unknown(UnknownReason::Unimplemented),
        intrinsics: vec![],
        budget_unroll: 0,
        budget_solver_steps: 0,
        checker_name: "equiv".into(),
        checker_version: "0.0.1".into(),
    }
}

#[test]
fn sign_and_verify_roundtrip() {
    let key = SigningKey::generate();
    let signed = SignedReceipt::sign(sample_receipt().to_bytes(), &key);
    assert!(signed.verify());
    assert_eq!(signed.public_key, key.public_key());
}

#[test]
fn envelope_roundtrips_through_bytes() {
    let key = SigningKey::generate();
    let signed = SignedReceipt::sign(sample_receipt().to_bytes(), &key);
    let bytes = signed.to_bytes();
    let parsed = SignedReceipt::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, signed);
    assert!(parsed.verify());
}

#[test]
fn tampered_receipt_fails_verification() {
    let key = SigningKey::generate();
    let mut signed = SignedReceipt::sign(sample_receipt().to_bytes(), &key);
    // Flip a byte in the receipt payload; signature must no longer hold.
    signed.receipt[10] ^= 0xff;
    assert!(!signed.verify());
}

#[test]
fn wrong_key_fails() {
    let a = SigningKey::generate();
    let b = SigningKey::generate();
    let signed = SignedReceipt::sign(sample_receipt().to_bytes(), &a);
    assert!(!verify(&signed.receipt, &signed.signature, &b.public_key()));
}

#[test]
fn signing_is_deterministic() {
    // ed25519 (RFC 8032) is deterministic: same seed + same bytes => same sig.
    // This is what keeps signed receipts byte-identical (AC-1 preserved).
    let seed = [7u8; 32];
    let k1 = SigningKey::from_seed(&seed);
    let k2 = SigningKey::from_seed(&seed);
    let msg = sample_receipt().to_bytes();
    let s1 = SignedReceipt::sign(msg.clone(), &k1);
    let s2 = SignedReceipt::sign(msg, &k2);
    assert_eq!(s1.to_bytes(), s2.to_bytes(), "signed receipts must be reproducible");
    assert_eq!(s1.receipt_id(), s2.receipt_id());
}

#[test]
fn hex_seed_roundtrip() {
    let key = SigningKey::generate();
    let h: String = key.seed().iter().map(|b| format!("{b:02x}")).collect();
    let key2 = SigningKey::from_hex(&h).unwrap();
    assert_eq!(key.public_key(), key2.public_key());
}
