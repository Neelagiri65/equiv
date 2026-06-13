//! in-toto attestation layer: well-formed JSON, correct schema, deterministic,
//! and a DSSE-signed roundtrip (offline; the keyless cert binding is cosign's
//! job in CI, but the signature over the DSSE PAE is verifiable here).
use equiv_core::attest::{dsse_pae, statement, ReviewPredicate, DSSE_PAYLOAD_TYPE, PREDICATE_TYPE};
use equiv_core::sign::{verify, SigningKey};

fn sample_statement() -> String {
    let pred = ReviewPredicate {
        verdict: "equivalent",
        detail: "agreed on 300 cases",
        receipt_id_hex: "e0f28612a413",
        checker: "equiv-review",
        checker_version: "0.0.1",
    };
    statement("src/math.py::total", "abcd1234", &pred.to_json())
}

#[test]
fn statement_is_well_formed_intoto_v1() {
    let s = sample_statement();
    let v: serde_json::Value = serde_json::from_str(&s).expect("must be valid JSON");
    assert_eq!(v["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(v["predicateType"], PREDICATE_TYPE);
    assert_eq!(v["subject"][0]["name"], "src/math.py::total");
    assert_eq!(v["subject"][0]["digest"]["sha256"], "abcd1234");
    assert_eq!(v["predicate"]["verdict"], "equivalent");
    assert_eq!(v["predicate"]["receiptId"], "e0f28612a413");
    assert_eq!(v["predicate"]["checker"]["name"], "equiv-review");
}

#[test]
fn statement_is_deterministic() {
    assert_eq!(sample_statement(), sample_statement());
}

#[test]
fn string_escaping_cannot_break_json() {
    // A hostile "function name" with quotes/newlines must stay valid JSON.
    let pred = ReviewPredicate {
        verdict: "error",
        detail: "boom\"}\n{\"x",
        receipt_id_hex: "00",
        checker: "equiv-review",
        checker_version: "0.0.1",
    };
    let s = statement("evil\"name\nx", "ff", &pred.to_json());
    let v: serde_json::Value = serde_json::from_str(&s).expect("escaped JSON must parse");
    assert_eq!(v["subject"][0]["name"], "evil\"name\nx");
}

#[test]
fn dsse_pae_framing_is_correct() {
    let payload = b"{}";
    let pae = dsse_pae(DSSE_PAYLOAD_TYPE, payload);
    let expected = format!("DSSEv1 {} {} {} {{}}", DSSE_PAYLOAD_TYPE.len(), DSSE_PAYLOAD_TYPE, payload.len());
    assert_eq!(pae, expected.as_bytes());
}

#[test]
fn dsse_signature_roundtrips_offline() {
    // The DSSE bytes are signable/verifiable with a plain key offline; in CI,
    // cosign does this keyless with a Fulcio cert instead of a raw key.
    let key = SigningKey::generate();
    let stmt = sample_statement();
    let pae = dsse_pae(DSSE_PAYLOAD_TYPE, stmt.as_bytes());
    let sig = key.sign(&pae);
    assert!(verify(&pae, &sig, &key.public_key()));
    // Tamper the statement -> PAE changes -> signature no longer holds.
    let pae2 = dsse_pae(DSSE_PAYLOAD_TYPE, b"{\"_type\":\"forged\"}");
    assert!(!verify(&pae2, &sig, &key.public_key()));
}
