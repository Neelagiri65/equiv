//! Founding-principle constraint test (P2 / AC-3): no certificate, no
//! `proved`. An UNSAT answer from the solver is not evidence anyone else can
//! check, so a default build must never emit `Verdict::Proved`. A correct
//! artifact the BMC path would have "proved" must come back as tested-N from
//! inputs that actually ran. Lift this only together with an independent
//! DRAT/LRAT certificate checker.
#![cfg(not(feature = "uncertified-proofs"))]

use equiv_core::parse_contract;
use equiv_core::verdict::Verdict;
use equiv_engine::{check, DiffConfig};

const ADD_CONTRACT: &str = r#"
(contract eqc/0
  (target (func "add" (param $a i32) (param $b i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (and (le_u $a (c32 1000000)) (le_u $b (c32 1000000))))
  (post (eq result (add $a $b)))
  (frame))
"#;

const ADD_OK: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "add") (param i32 i32) (result i32)
    (i32.add (local.get 0) (local.get 1))))
"#;

const ADD_BUGGY: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "add") (param i32 i32) (result i32)
    (i32.sub (local.get 0) (local.get 1))))
"#;

#[test]
fn unsat_without_certificate_is_never_proved() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wat::parse_str(ADD_OK).unwrap(), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::TestedN { .. }),
        "uncertified UNSAT must fall back to tested-N, got {:?}",
        r.verdict
    );
}

#[test]
fn solver_counterexamples_still_reported() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wat::parse_str(ADD_BUGGY).unwrap(), &c, &DiffConfig::default());
    assert!(matches!(r.verdict, Verdict::Counterexample { .. }), "got {:?}", r.verdict);
}
