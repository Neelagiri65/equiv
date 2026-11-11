//! equiv-engine: the checking pipeline. v0 = static WASM analysis + the
//! deterministic differential tester. The BMC/SMT proving path lands behind
//! the same `check` signature; until then no `proved` verdict can exist
//! (AC-3, enforced in equiv-core).

pub mod bmc;
pub mod difftest;
pub mod eval;
pub mod wasm;

use equiv_core::verdict::{Receipt, Verdict};
use equiv_core::{Contract, UnknownReason};

pub use difftest::DiffConfig;
pub use wasm::FuncSig;

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// A receipt plus, when there is something to add, a sentence for people:
/// what could not be checked (verdict `Unknown`), or how many drawn inputs
/// the precondition excluded (tested-N). It is not hashed into the receipt.
pub struct Checked {
    pub receipt: Receipt,
    pub note: Option<String>,
}

/// Check an artifact against a contract. v0 pipeline:
/// 1. static module analysis; assume-flag violations end the check
/// 2. clauses and signatures this version cannot evaluate end the check
/// 3. symbolic path, else deterministic differential testing
pub fn check(artifact: &[u8], contract: &Contract, cfg: &DiffConfig) -> Receipt {
    check_explained(artifact, contract, cfg).receipt
}

/// `check`, with the reason for any `Unknown` verdict.
pub fn check_explained(artifact: &[u8], contract: &Contract, cfg: &DiffConfig) -> Checked {
    let receipt = |verdict: Verdict| Receipt {
        artifact_sha256: sha256(artifact),
        contract_sha256: equiv_core::contract_sha256(contract),
        reference_sha256: contract.refines.as_ref().map(|r| r.artifact_sha256),
        verdict,
        intrinsics: Vec::new(),
        budget_unroll: 0,
        budget_solver_steps: 0,
        checker_name: "equiv".into(),
        checker_version: env!("CARGO_PKG_VERSION").into(),
    };
    let unknown = |reason: UnknownReason, why: String| Checked {
        receipt: receipt(Verdict::Unknown(reason)),
        note: Some(why),
    };
    let target = &contract.target;

    let facts = match wasm::analyze(artifact) {
        Ok(f) => f,
        Err(wasm::WasmError::Parse(e)) => {
            return unknown(
                UnknownReason::IllFormedExpr,
                format!("the artifact is not a readable WASM module: {e}"),
            )
        }
    };
    if facts.export_sig(&target.name).is_none() {
        return unknown(
            UnknownReason::IllFormedExpr,
            format!("the artifact exports no function named `{}`", target.name),
        );
    }
    // Spec §10 Q4 (provisional v0 answer): a module that statically violates
    // its assume-flags yields `unknown`, not `counterexample`: the caller
    // lied about the artifact, the property was never tested.
    let violated = wasm::violated_flags(&facts, &contract.assume);
    if !violated.is_empty() {
        return unknown(
            UnknownReason::UnsupportedFeature,
            format!("the artifact contradicts the contract's assume flags: {violated:?}"),
        );
    }
    // `refines` names a reference artifact this version never loads. Running
    // only pre/post would put that artifact's hash on a receipt for a
    // comparison that did not happen.
    if contract.refines.is_some() {
        return unknown(
            UnknownReason::UnsupportedFeature,
            "refines is not evaluated by this version: the reference artifact was not loaded and nothing              was compared against it"
                .into(),
        );
    }
    if let Some((reason, why)) = wasm::signature_problem(&facts, target) {
        return unknown(reason, why);
    }

    // Symbolic path first (loop-free scalar envelope; floats never reach it
    // since float signatures fail the scalar check; spec §6.3 routing).
    let outcome = bmc::try_prove(artifact, contract);
    if let Some(verdict) = bmc::outcome_verdict(outcome) {
        let note = match &verdict {
            Verdict::Unknown(UnknownReason::VacuousPre) => {
                Some("no input satisfies the precondition, so the contract says nothing about the artifact".to_string())
            }
            Verdict::Unknown(UnknownReason::SolverDisagreement) => Some(
                "the solver's counterexample did not reproduce when the artifact was run on it; this is a                  checker fault, not a finding about the artifact"
                    .to_string(),
            ),
            _ => None,
        };
        let mut r = receipt(verdict);
        if let Verdict::Proved(_) = &r.verdict {
            r.budget_solver_steps = match &r.verdict {
                Verdict::Proved(ev) => ev.solver_steps(),
                _ => 0,
            };
        }
        return Checked { receipt: r, note };
    }
    // Fallback: deterministic differential testing.
    let (verdict, note) = difftest::difftest_explained(artifact, contract, cfg);
    Checked {
        receipt: receipt(verdict),
        note,
    }
}
