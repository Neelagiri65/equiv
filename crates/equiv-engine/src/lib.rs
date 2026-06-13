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

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// Check an artifact against a contract. v0 pipeline:
/// 1. static module analysis; assume-flag violations end the check
/// 2. float routing (spec §6.3): floats anywhere => differential only
/// 3. deterministic differential testing => tested-N / counterexample
pub fn check(artifact: &[u8], contract: &Contract, cfg: &DiffConfig) -> Receipt {
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

    let facts = match wasm::analyze(artifact) {
        Ok(f) => f,
        Err(_) => return receipt(Verdict::Unknown(UnknownReason::IllFormedExpr)),
    };
    if !facts.exported_funcs.iter().any(|f| f == &contract.target.name) {
        return receipt(Verdict::Unknown(UnknownReason::IllFormedExpr));
    }
    // Spec §10 Q4 (provisional v0 answer): a module that statically violates
    // its assume-flags yields `unknown`, not `counterexample` — the caller
    // lied about the artifact, the property was never tested.
    if !wasm::violated_flags(&facts, &contract.assume).is_empty() {
        return receipt(Verdict::Unknown(UnknownReason::UnsupportedFeature));
    }

    // Symbolic path first (loop-free scalar envelope; floats never reach it
    // since float signatures fail the scalar check — spec §6.3 routing).
    let outcome = bmc::try_prove(artifact, contract);
    if let Some(verdict) = bmc::outcome_verdict(outcome) {
        let mut r = receipt(verdict);
        if let Verdict::Proved(_) = &r.verdict {
            r.budget_solver_steps = match &r.verdict {
                Verdict::Proved(ev) => ev.solver_steps(),
                _ => 0,
            };
        }
        return r;
    }
    // Fallback: deterministic differential testing.
    receipt(difftest::difftest(artifact, contract, cfg))
}
