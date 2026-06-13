//! Verdicts and receipts.
//!
//! AC-3 ("never `proved` for `tested`") is enforced structurally: a
//! `Verdict::Proved` can only be built through `ProofEvidence`, whose only
//! constructor is `pub(crate)` and lives in the engine module. CLI code,
//! tests, and external crates cannot fabricate a proved verdict.

use crate::cbor::Value;

/// Witness that the SMT obligation set was discharged. The only way to obtain
/// one is from the engine (crate-internal). Until the engine exists, no
/// `Proved` verdict can exist anywhere in the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofEvidence {
    pub(crate) unroll_k: u32,
    pub(crate) solver_steps: u64,
}

impl ProofEvidence {
    /// Mint evidence from an UNSAT result. The ONLY sanctioned call site is
    /// the BMC module in equiv-engine, immediately after the solver returns
    /// UNSAT on `pre ∧ ¬post`; a conformance test greps the workspace to pin
    /// the call-site count. Hidden from docs to keep the API surface honest.
    #[doc(hidden)]
    pub fn mint_unsat(unroll_k: u32, solver_steps: u64) -> Self {
        ProofEvidence { unroll_k, solver_steps }
    }

    pub fn unroll_k(&self) -> u32 {
        self.unroll_k
    }

    pub fn solver_steps(&self) -> u64 {
        self.solver_steps
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    VacuousPre,
    BoundUnprovable,
    UnsupportedFeature,
    IllFormedExpr,
    SolverDisagreement,
    BudgetExceeded,
    /// Pre-v1 only: the proving engine is not implemented yet.
    Unimplemented,
}

impl UnknownReason {
    pub fn code(self) -> u64 {
        match self {
            UnknownReason::VacuousPre => 0,
            UnknownReason::BoundUnprovable => 1,
            UnknownReason::UnsupportedFeature => 2,
            UnknownReason::IllFormedExpr => 3,
            UnknownReason::SolverDisagreement => 4,
            UnknownReason::BudgetExceeded => 5,
            UnknownReason::Unimplemented => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Proved(ProofEvidence),
    Counterexample {
        /// Concrete argument values (i32/i64 widened to u64).
        args: Vec<u64>,
        /// True if the violation is a trap not permitted by the contract.
        trap: bool,
    },
    TestedN {
        n_cases: u64,
        seed: u64,
    },
    Unknown(UnknownReason),
}

impl Verdict {
    /// DR-7 frozen exit-code contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            Verdict::Proved(_) | Verdict::TestedN { .. } => 0,
            Verdict::Counterexample { .. } => 1,
            Verdict::Unknown(_) => 2,
        }
    }

    fn value(&self) -> Value {
        match self {
            Verdict::Proved(ev) => Value::Array(vec![
                Value::Uint(0),
                Value::Uint(ev.unroll_k as u64),
                Value::Uint(ev.solver_steps),
            ]),
            Verdict::Counterexample { args, trap } => Value::Array(vec![
                Value::Uint(1),
                Value::Array(args.iter().map(|a| Value::Uint(*a)).collect()),
                Value::Uint(*trap as u64),
            ]),
            Verdict::TestedN { n_cases, seed } => {
                Value::Array(vec![Value::Uint(2), Value::Uint(*n_cases), Value::Uint(*seed)])
            }
            Verdict::Unknown(r) => Value::Array(vec![Value::Uint(3), Value::Uint(r.code())]),
        }
    }
}

/// The receipt: a canonical, hashable record of exactly what was checked,
/// under which abstractions and budgets, by which checker build. Signing
/// (ed25519) is deferred to v0.1; the format reserves the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub artifact_sha256: [u8; 32],
    pub contract_sha256: [u8; 32],
    pub reference_sha256: Option<[u8; 32]>,
    pub verdict: Verdict,
    /// Assume-flag codes of intrinsics actually applied (AC-8).
    pub intrinsics: Vec<u64>,
    pub budget_unroll: u32,
    pub budget_solver_steps: u64,
    pub checker_name: String,
    pub checker_version: String,
}

const RK_VERSION: u64 = 0;
const RK_ARTIFACT: u64 = 1;
const RK_CONTRACT: u64 = 2;
const RK_REFERENCE: u64 = 3;
const RK_VERDICT: u64 = 4;
const RK_INTRINSICS: u64 = 5;
const RK_BUDGETS: u64 = 6;
const RK_CHECKER: u64 = 7;

impl Receipt {
    /// Canonical bytes. Same inputs => byte-identical output (AC-1); no
    /// wall-clock, no randomness, no host-dependent field exists by design.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut entries: Vec<(u64, Value)> = vec![
            (RK_VERSION, Value::Uint(0)),
            (RK_ARTIFACT, Value::Bytes(self.artifact_sha256.to_vec())),
            (RK_CONTRACT, Value::Bytes(self.contract_sha256.to_vec())),
        ];
        if let Some(r) = &self.reference_sha256 {
            entries.push((RK_REFERENCE, Value::Bytes(r.to_vec())));
        }
        entries.push((RK_VERDICT, self.verdict.value()));
        let mut intr = self.intrinsics.clone();
        intr.sort_unstable();
        intr.dedup();
        entries.push((
            RK_INTRINSICS,
            Value::Array(intr.into_iter().map(Value::Uint).collect()),
        ));
        entries.push((
            RK_BUDGETS,
            Value::Array(vec![
                Value::Uint(self.budget_unroll as u64),
                Value::Uint(self.budget_solver_steps),
            ]),
        ));
        entries.push((
            RK_CHECKER,
            Value::Array(vec![
                Value::Text(self.checker_name.clone()),
                Value::Text(self.checker_version.clone()),
            ]),
        ));
        crate::cbor::to_bytes(&Value::Map(entries))
    }

    pub fn sha256(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.to_bytes());
        h.finalize().into()
    }
}
