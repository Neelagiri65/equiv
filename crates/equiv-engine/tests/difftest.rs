//! Engine integration tests over real WASM artifacts (WAT fixtures).
//! Covers: tested-N on a correct artifact, counterexample on a buggy one,
//! trap-as-counterexample, frame violation, assume-flag enforcement, and
//! determinism of the whole pipeline (AC-1).

use equiv_core::parse_contract;
use equiv_core::verdict::Verdict;
use equiv_engine::{check, DiffConfig};

fn wasm(wat_src: &str) -> Vec<u8> {
    wat::parse_str(wat_src).expect("fixture WAT must assemble")
}

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

/// The difftest component itself (the full pipeline would PROVE this
/// artifact via BMC; here we exercise the fallback path directly).
#[test]
fn correct_artifact_gets_tested_n() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let verdict = equiv_engine::difftest::difftest(&wasm(ADD_OK), &c, &DiffConfig::default());
    match verdict {
        Verdict::TestedN { n_cases, .. } => assert!(n_cases >= 32, "got only {n_cases} cases"),
        v => panic!("expected tested-N, got {v:?}"),
    }
    assert_eq!(verdict.exit_code(), 0);
}

#[test]
fn buggy_artifact_gets_counterexample() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wasm(ADD_BUGGY), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Counterexample { args, trap } => {
            assert!(!trap);
            assert_eq!(args.len(), 2);
            // The counterexample must actually violate the post: a + b != a - b
            // whenever b != 0 (mod 2^32 wrapping).
            assert_ne!(
                (args[0] as u32).wrapping_add(args[1] as u32),
                (args[0] as u32).wrapping_sub(args[1] as u32),
                "reported counterexample does not violate the contract"
            );
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
    assert_eq!(r.verdict.exit_code(), 1);
}

#[test]
fn trap_under_satisfied_pre_is_counterexample() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let trapping = r#"
(module
  (memory (export "memory") 1)
  (func (export "add") (param i32 i32) (result i32)
    unreachable))
"#;
    let r = check(&wasm(trapping), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::Counterexample { trap: true, .. }),
        "expected trap counterexample, got {:?}",
        r.verdict
    );
}

#[test]
fn frame_violation_is_counterexample() {
    // Contract: write byte 42 at $p, touch nothing else.
    let contract = r#"
(contract eqc/0
  (target (func "store42" (param $p i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (lt_u $p (c32 65536)))
  (post (eq (byte new $p) (c32 42)))
  (frame (range $p (c32 1))))
"#;
    let ok = r#"
(module
  (memory (export "memory") 1)
  (func (export "store42") (param i32)
    (i32.store8 (local.get 0) (i32.const 42))))
"#;
    // Violator also clobbers byte 0 (and still satisfies the post).
    let clobbers = r#"
(module
  (memory (export "memory") 1)
  (func (export "store42") (param i32)
    (i32.store8 (local.get 0) (i32.const 42))
    (i32.store8 (i32.const 0) (i32.const 7))))
"#;
    let c = parse_contract(contract).unwrap();
    let r_ok = check(&wasm(ok), &c, &DiffConfig::default());
    assert!(
        matches!(r_ok.verdict, Verdict::TestedN { .. }),
        "correct store should pass: {:?}",
        r_ok.verdict
    );
    let r_bad = check(&wasm(clobbers), &c, &DiffConfig::default());
    assert!(
        matches!(r_bad.verdict, Verdict::Counterexample { trap: false, .. }),
        "frame clobber must be caught: {:?}",
        r_bad.verdict
    );
}

#[test]
fn assume_flag_violation_is_unknown() {
    // Module imports a function; contract says no-imports.
    let importing = r#"
(module
  (import "env" "f" (func))
  (memory (export "memory") 1)
  (func (export "add") (param i32 i32) (result i32)
    (i32.add (local.get 0) (local.get 1))))
"#;
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wasm(importing), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::Unknown(_)),
        "flag violation must be unknown, got {:?}",
        r.verdict
    );
}

#[test]
fn missing_export_is_unknown() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let no_export = r#"(module (memory (export "memory") 1))"#;
    let r = check(&wasm(no_export), &c, &DiffConfig::default());
    assert!(matches!(r.verdict, Verdict::Unknown(_)));
}

/// AC-1 over the full pipeline: identical inputs => byte-identical receipts,
/// including the sampled test vectors (seeded PRNG, no host randomness).
#[test]
fn ac1_pipeline_determinism() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let a = wasm(ADD_OK);
    let r1 = check(&a, &c, &DiffConfig::default());
    let r2 = check(&a, &c, &DiffConfig::default());
    assert_eq!(r1.to_bytes(), r2.to_bytes());

    let b1 = check(&wasm(ADD_BUGGY), &c, &DiffConfig::default());
    let b2 = check(&wasm(ADD_BUGGY), &c, &DiffConfig::default());
    assert_eq!(b1.to_bytes(), b2.to_bytes(), "counterexamples must be reproducible");
}

/// AC-7 (difftest slice): shrinking the case budget can only weaken the
/// verdict (fewer cases), never flip pass/fail direction.
#[test]
fn ac7_budget_monotonicity() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let small = DiffConfig {
        n_cases: 8,
        ..DiffConfig::default()
    };
    let v_small = equiv_engine::difftest::difftest(&wasm(ADD_OK), &c, &small);
    let v_big = equiv_engine::difftest::difftest(&wasm(ADD_OK), &c, &DiffConfig::default());
    match (&v_small, &v_big) {
        (Verdict::TestedN { n_cases: ns, .. }, Verdict::TestedN { n_cases: nb, .. }) => {
            assert!(ns <= nb)
        }
        v => panic!("expected tested-N at both budgets, got {v:?}"),
    }
}
