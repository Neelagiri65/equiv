//! in-toto attestation primitives (DR-3) — the format the keyless Sigstore
//! path signs.
//!
//! equiv owns the *payload* (a standard in-toto Statement wrapping the review
//! receipt as a predicate). The keyless crypto — OIDC → Fulcio short-lived
//! cert → sign → Rekor transparency log — is delegated to `cosign`, the
//! standard, audited Sigstore client (we do NOT reimplement Fulcio/Rekor).
//!
//! Statement shape (in-toto v1):
//!   { "_type": "https://in-toto.io/Statement/v1",
//!     "subject": [{ "name": ..., "digest": { "sha256": ... } }],
//!     "predicateType": "https://equiv.dev/attestations/review/v1",
//!     "predicate": { ... } }
//!
//! cosign signs this as a DSSE envelope; `dsse_pae` exposes the exact bytes
//! that get signed, for offline verification/testing.

pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
pub const PREDICATE_TYPE: &str = "https://equiv.dev/attestations/review/v1";
pub const DSSE_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

/// Minimal RFC 8259 string escaping (we only emit known-shape strings, but
/// escape properly so a function name with quotes can't break the JSON).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn obj(pairs: &[(&str, String)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("\"{}\":{}", esc(k), v))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn jstr(s: &str) -> String {
    format!("\"{}\"", esc(s))
}

/// A review predicate (the equiv-specific body of the attestation).
pub struct ReviewPredicate<'a> {
    pub verdict: &'a str,
    pub detail: &'a str,
    pub receipt_id_hex: &'a str,
    pub checker: &'a str,
    pub checker_version: &'a str,
}

impl ReviewPredicate<'_> {
    pub fn to_json(&self) -> String {
        obj(&[
            ("verdict", jstr(self.verdict)),
            ("detail", jstr(self.detail)),
            ("receiptId", jstr(self.receipt_id_hex)),
            (
                "checker",
                obj(&[
                    ("name", jstr(self.checker)),
                    ("version", jstr(self.checker_version)),
                ]),
            ),
        ])
    }
}

/// Build an in-toto v1 Statement. `subject_name` identifies what was reviewed
/// (e.g. "src/math.py::total"); `subject_sha256_hex` is its content digest.
/// Field order is fixed, so the same inputs yield byte-identical JSON.
pub fn statement(subject_name: &str, subject_sha256_hex: &str, predicate_json: &str) -> String {
    let subject = format!(
        "[{}]",
        obj(&[
            ("name", jstr(subject_name)),
            ("digest", obj(&[("sha256", jstr(subject_sha256_hex))])),
        ])
    );
    obj(&[
        ("_type", jstr(STATEMENT_TYPE)),
        ("subject", subject),
        ("predicateType", jstr(PREDICATE_TYPE)),
        ("predicate", predicate_json.to_string()),
    ])
}

/// DSSE Pre-Authentication Encoding (the exact bytes a DSSE signature covers):
///   "DSSEv1 " + len(type) + " " + type + " " + len(payload) + " " + payload
pub fn dsse_pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload);
    out
}
