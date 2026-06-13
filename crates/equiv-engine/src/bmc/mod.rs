//! Bounded model checking (v0): loop-free scalar functions, monolithic
//! formula `pre ∧ ¬post`, bit-blasted to SAT. UNSAT mints ProofEvidence —
//! the only place in the system allowed to (AC-3; pinned by a source-grep
//! test). SAT models are replayed concretely before being reported (AC-4).

mod blast;
mod sym;
mod term;

use equiv_core::ast::{BvOp, Contract, Expr, ValType};
use equiv_core::verdict::{ProofEvidence, Verdict};
use term::{Arena, TId, Term};

pub enum Outcome {
    Proved(ProofEvidence),
    /// SAT model, already replayed and confirmed against the interpreter.
    Counterexample { args: Vec<u64> },
    /// Model failed concrete replay: solver/encoder disagreement — a bug,
    /// surfaced honestly, never reported as a counterexample.
    Disagreement,
    /// Precondition is UNSAT: the contract proves nothing (AC-6).
    VacuousPre,
    /// A loop exceeded the unroll bound on some pre-satisfying path; no
    /// concrete violation found. Caller falls back to difftest.
    BoundExceeded,
    /// Outside the v0 BMC envelope; caller falls back to difftest.
    Unsuitable,
}

/// Default unroll bound (spec envelope: k <= 64).
pub const UNROLL_K: u32 = 64;

/// Coverage diagnostic: classify what the BMC front-end does with an
/// exported function, independent of any contract. Measures the REAL
/// proving-path accept-rate (the number the devil's-advocate review claimed
/// was ~0% on real compiled output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Extracted AND symbolically executed: eligible for `proved`.
    Provable,
    /// Found, but uses an opcode/shape outside the scalar envelope
    /// (memory, calls, br_table, div/rem, sign-ext, floats, multi-value...).
    OutOfEnvelope,
    /// Export not present.
    NotFound,
}

pub fn coverage_probe(wasm_bytes: &[u8], export_name: &str) -> Coverage {
    let Some(body) = sym::extract(wasm_bytes, export_name) else {
        // extract() returns None both for "not found" and "unsupported op".
        // Disambiguate via a cheap export scan.
        let found = crate::wasm::analyze(wasm_bytes)
            .map(|f| f.exported_funcs.iter().any(|n| n == export_name))
            .unwrap_or(false);
        return if found {
            Coverage::OutOfEnvelope
        } else {
            Coverage::NotFound
        };
    };
    let mut ar = Arena::default();
    match sym::execute(&body, UNROLL_K, &mut ar) {
        Ok(_) => Coverage::Provable,
        Err(_) => Coverage::OutOfEnvelope,
    }
}

/// Translate a scalar contract expression to a term. Any memory/region
/// construct, forall, div/rem, or width mismatch makes the contract
/// unsuitable for the v0 symbolic path.
fn tr(e: &Expr, ar: &mut Arena, params: &[ValType], result: Option<TId>) -> Result<TId, ()> {
    Ok(match e {
        Expr::C32(v) => ar.c(32, *v as u64),
        Expr::C64(v) => ar.c(64, *v),
        Expr::Arg(i) => {
            let t = params.get(*i as usize).ok_or(())?;
            let w = match t {
                ValType::I64 => 64,
                ValType::I32 => 32,
                _ => return Err(()),
            };
            ar.add(Term::Arg { i: *i, w })
        }
        Expr::Result => result.ok_or(())?,
        Expr::Bv(op, a, b) => {
            if matches!(op, BvOp::DivU | BvOp::DivS | BvOp::RemU | BvOp::RemS) {
                return Err(());
            }
            let at = tr(a, ar, params, result)?;
            let bt = tr(b, ar, params, result)?;
            let w = ar.width(at);
            if ar.width(bt) != w {
                return Err(());
            }
            ar.add(Term::Bin { op: *op, w, a: at, b: bt })
        }
        Expr::Zext(a) => {
            let at = tr(a, ar, params, result)?;
            if ar.width(at) != 32 {
                return Err(());
            }
            ar.add(Term::Ext { a: at, w: 64, signed: false })
        }
        Expr::Wrap(a) => {
            let at = tr(a, ar, params, result)?;
            if ar.width(at) != 64 {
                return Err(());
            }
            ar.add(Term::Slice { a: at, w: 32 })
        }
        Expr::Cmp(op, a, b) => {
            let at = tr(a, ar, params, result)?;
            let bt = tr(b, ar, params, result)?;
            if ar.width(at) != ar.width(bt) {
                return Err(());
            }
            ar.add(Term::Cmp { op: *op, a: at, b: bt })
        }
        Expr::Band(a, b) => {
            let at = tr_bool(a, ar, params, result)?;
            let bt = tr_bool(b, ar, params, result)?;
            ar.band(at, bt)
        }
        Expr::Bor(a, b) => {
            let at = tr_bool(a, ar, params, result)?;
            let bt = tr_bool(b, ar, params, result)?;
            ar.bor(at, bt)
        }
        Expr::Bnot(a) => {
            let at = tr_bool(a, ar, params, result)?;
            ar.bnot(at)
        }
        Expr::Implies(a, b) => {
            let at = tr_bool(a, ar, params, result)?;
            let bt = tr_bool(b, ar, params, result)?;
            let na = ar.bnot(at);
            ar.bor(na, bt)
        }
        Expr::Ite(c, a, b) => {
            let ct = tr_bool(c, ar, params, result)?;
            let at = tr(a, ar, params, result)?;
            let bt = tr(b, ar, params, result)?;
            let w = ar.width(at);
            if ar.width(bt) != w {
                return Err(());
            }
            ar.add(Term::Ite { c: ct, a: at, b: bt, w })
        }
        // Memory, regions, forall: not in the scalar envelope.
        _ => return Err(()),
    })
}

/// Coerce a term into boolean position (1-bit): comparisons stay; wider
/// terms get the `!= 0` reading, matching the concrete evaluator.
fn tr_bool(e: &Expr, ar: &mut Arena, params: &[ValType], result: Option<TId>) -> Result<TId, ()> {
    let t = tr(e, ar, params, result)?;
    Ok(if ar.width(t) == 1 {
        t
    } else {
        ar.add(Term::NeZero { a: t })
    })
}

fn scalar_types(types: &[ValType]) -> bool {
    types.iter().all(|t| matches!(t, ValType::I32 | ValType::I64))
}

pub fn try_prove(wasm_bytes: &[u8], contract: &Contract) -> Outcome {
    // Contract-side envelope: scalar signature, empty frame, no refines.
    if !contract.frame.is_empty()
        || contract.refines.is_some()
        || !scalar_types(&contract.target.params)
        || !scalar_types(&contract.target.results)
    {
        return Outcome::Unsuitable;
    }
    // Artifact-side envelope: extraction fails on any unsupported operator.
    let Some(body) = sym::extract(wasm_bytes, &contract.target.name) else {
        return Outcome::Unsuitable;
    };
    if body.params != contract.target.params || body.n_results != contract.target.results.len() {
        return Outcome::Unsuitable;
    }

    let mut ar = Arena::default();
    let Ok(symres) = sym::execute(&body, UNROLL_K, &mut ar) else {
        return Outcome::Unsuitable;
    };

    let Ok(pre) = tr_bool(&contract.pre, &mut ar, &body.params, None) else {
        return Outcome::Unsuitable;
    };
    let Ok(post) = tr_bool(&contract.post, &mut ar, &body.params, symres.result) else {
        return Outcome::Unsuitable;
    };

    let fresh_args = |blaster: &mut blast::Blaster| -> Vec<Vec<varisat::Lit>> {
        body.params
            .iter()
            .map(|t| blaster.fresh_vec(if matches!(t, ValType::I64) { 64 } else { 32 }))
            .collect()
    };

    // AC-6 vacuity: an UNSAT precondition proves nothing.
    {
        let mut b = blast::Blaster::new();
        let args = fresh_args(&mut b);
        match b.solve_assert(&ar, pre, &args) {
            Ok(None) => return Outcome::VacuousPre,
            Ok(Some(_)) => {}
            Err(_) => return Outcome::Unsuitable,
        }
    }

    // Main obligation: pre ∧ (¬post ∨ trap ∨ bound-exceeded).
    // UNSAT discharges the postcondition, the no-trap default obligation,
    // AND the unwinding assertion in one query.
    let npost = ar.bnot(post);
    let mut bad = npost;
    if let Some(t) = symres.trap {
        bad = ar.bor(bad, t);
    }
    if let Some(u) = symres.incomplete {
        bad = ar.bor(bad, u);
    }
    let formula = ar.band(pre, bad);

    let mut blaster = blast::Blaster::new();
    let arg_bits = fresh_args(&mut blaster);
    let unroll_used = if symres.incomplete.is_some() || body.ops.iter().any(|o| matches!(o, sym::MiniOp::Loop)) {
        symres.unroll_k
    } else {
        0
    };
    match blaster.solve_assert(&ar, formula, &arg_bits) {
        Err(_) => Outcome::Unsuitable,
        Ok(None) => {
            // UNSAT: no pre-satisfying input violates the post, traps, or
            // outruns the bound. The one sanctioned minting site.
            Outcome::Proved(ProofEvidence::mint_unsat(unroll_used, blaster.clauses))
        }
        Ok(Some(args)) => match crate::difftest::replay_scalar(wasm_bytes, contract, &args) {
            Some(true) => Outcome::Counterexample { args },
            // Model doesn't violate concretely. With loops in play the
            // SAT witness may be a bound overflow, not a bug — honest
            // fallback. Loop-free, it's an encoder/solver bug.
            _ if symres.incomplete.is_some() => Outcome::BoundExceeded,
            _ => Outcome::Disagreement,
        },
    }
}

/// Wrap an outcome as a verdict; None means "fall back to difftest".
pub fn outcome_verdict(o: Outcome) -> Option<Verdict> {
    match o {
        Outcome::Proved(ev) => Some(Verdict::Proved(ev)),
        Outcome::Counterexample { args } => Some(Verdict::Counterexample { args, trap: false }),
        Outcome::Disagreement => Some(Verdict::Unknown(
            equiv_core::UnknownReason::SolverDisagreement,
        )),
        Outcome::VacuousPre => Some(Verdict::Unknown(equiv_core::UnknownReason::VacuousPre)),
        Outcome::BoundExceeded | Outcome::Unsuitable => None,
    }
}
