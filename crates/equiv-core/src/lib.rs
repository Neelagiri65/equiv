//! equiv-core: the eqc contract format — AST, canonical encoding, text
//! surface, verdicts, receipts. No engine lives here yet; until it does, the
//! only verdicts the system can produce are non-`proved` (AC-3 by
//! construction).

pub mod ast;
pub mod attest;
pub mod cbor;
pub mod codec;
pub mod sign;
pub mod text;
pub mod verdict;

pub use ast::Contract;
pub use codec::{contract_sha256, decode_contract, encode_contract, DecodeError};
pub use sign::{SignedReceipt, SigningKey};
pub use text::parse_contract;
pub use verdict::{Receipt, UnknownReason, Verdict};

/// Placeholder check entry point: parses inputs and returns an honest
/// `unknown(unimplemented)` receipt. Exists so the CLI, hooks, and receipt
/// plumbing are real end-to-end before the engine lands behind this exact
/// signature.
pub fn check_stub(artifact_bytes: &[u8], contract: &Contract) -> Receipt {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(artifact_bytes);
    let artifact_sha256: [u8; 32] = h.finalize().into();
    Receipt {
        artifact_sha256,
        contract_sha256: contract_sha256(contract),
        reference_sha256: contract.refines.as_ref().map(|r| r.artifact_sha256),
        verdict: Verdict::Unknown(UnknownReason::Unimplemented),
        intrinsics: Vec::new(),
        budget_unroll: 0,
        budget_solver_steps: 0,
        checker_name: "equiv".into(),
        checker_version: env!("CARGO_PKG_VERSION").into(),
    }
}
