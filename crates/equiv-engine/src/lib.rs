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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
    check_with_reference(artifact, None, contract, cfg)
}

/// `check_explained`, with the reference artifact a `refines` clause names.
/// `reference` is the file the caller offers as that artifact; it is hashed
/// here and must match the hash in the contract.
pub fn check_with_reference(
    artifact: &[u8],
    reference: Option<&[u8]>,
    contract: &Contract,
    cfg: &DiffConfig,
) -> Checked {
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
    if let Some((reason, why)) = wasm::signature_problem(&facts, target) {
        return unknown(reason, why);
    }
    // `refines`: the target is compared with a reference artifact on the
    // same inputs. The receipt carries that artifact's hash, so the
    // comparison must really have run against exactly those bytes: anything
    // short of that is could-not-check, never a pass on pre/post alone.
    let mut compare: Option<difftest::Reference> = None;
    if let Some(r) = &contract.refines {
        let Some(bytes) = reference else {
            return unknown(
                UnknownReason::UnsupportedFeature,
                "the contract's refines clause names a reference artifact but no reference was supplied, so nothing \
                 was compared against it"
                    .into(),
            );
        };
        if sha256(bytes) != r.artifact_sha256 {
            return unknown(
                UnknownReason::IllFormedExpr,
                format!(
                    "the file given as the reference has sha256 {} but the contract names {}; nothing was compared",
                    hex(&sha256(bytes)),
                    hex(&r.artifact_sha256)
                ),
            );
        }
        if r.assuming.is_some() {
            return unknown(
                UnknownReason::UnsupportedFeature,
                "the refines clause has an `assuming` part, which this version does not evaluate; nothing was compared"
                    .into(),
            );
        }
        // This version compares the two on the target's own arguments: the
        // `over` list must be the target's parameters, in order (a
        // `(range a b)` stands for the two arguments a and b).
        let mut flat: Vec<&equiv_core::ast::Expr> = Vec::new();
        for e in &r.over {
            match e {
                equiv_core::ast::Expr::Range(a, b) => {
                    flat.push(a);
                    flat.push(b);
                }
                other => flat.push(other),
            }
        }
        let identity = flat.len() == target.params.len()
            && flat.iter().enumerate().all(|(i, e)| matches!(e, equiv_core::ast::Expr::Arg(n) if *n as usize == i));
        if !identity {
            return unknown(
                UnknownReason::UnsupportedFeature,
                "the refines clause's `over` list is not the target's parameters in order; this version compares \
                 the two functions on the same arguments only, so nothing was compared"
                    .into(),
            );
        }
        if target.results.len() > 1 {
            return unknown(
                UnknownReason::UnsupportedFeature,
                "refines compares a single result; the target declares more than one, so nothing was compared".into(),
            );
        }
        let ref_target = equiv_core::ast::Target {
            name: r.func.clone(),
            params: target.params.clone(),
            results: target.results.clone(),
        };
        let ref_facts = match wasm::analyze(bytes) {
            Ok(f) => f,
            Err(wasm::WasmError::Parse(e)) => {
                return unknown(
                    UnknownReason::IllFormedExpr,
                    format!("the reference is not a readable WASM module: {e}; nothing was compared"),
                )
            }
        };
        if let Some((reason, why)) = wasm::signature_problem(&ref_facts, &ref_target) {
            return unknown(reason, format!("reference: {why}; nothing was compared"));
        }
        // The reference is run with nothing to import, as the target is.
        if ref_facts.func_imports > 0 {
            return unknown(
                UnknownReason::UnsupportedFeature,
                format!(
                    "the reference imports {} function(s); this version runs it with nothing to import, so \
                     nothing was compared",
                    ref_facts.func_imports
                ),
            );
        }
        if ref_facts.memories > 0 && !ref_facts.memory_exported && !bmc::touches_no_memory(bytes, &r.func) {
            return unknown(
                UnknownReason::UnsupportedFeature,
                "the reference has a linear memory but does not export it, so the bytes it writes cannot be \
                 compared; nothing was compared"
                    .into(),
            );
        }
        compare = Some(difftest::Reference { wasm: bytes, target: ref_target });
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
    let (verdict, note) = difftest::difftest_with_reference(artifact, contract, cfg, compare.as_ref());
    Checked {
        receipt: receipt(verdict),
        note,
    }
}
