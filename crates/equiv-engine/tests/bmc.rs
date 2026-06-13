//! BMC integration tests: the first `proved` verdicts in the system.

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

/// A correct artifact is proved, not merely tested.
#[test]
fn add_is_proved() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wasm(ADD_OK), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Proved(ev) => assert!(ev.solver_steps() > 0),
        v => panic!("expected proved, got {v:?}"),
    }
    assert_eq!(r.verdict.exit_code(), 0);
}

/// Buggy artifact: SAT model, replayed concretely, reported as cex.
#[test]
fn buggy_add_cex_via_solver() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let r = check(&wasm(ADD_BUGGY), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Counterexample { args, trap } => {
            assert!(!trap);
            // Must satisfy the pre (the solver was asked for pre ∧ ¬post).
            assert!(args[0] <= 1_000_000 && args[1] <= 1_000_000);
            assert_ne!(
                (args[0] as u32).wrapping_add(args[1] as u32),
                (args[0] as u32).wrapping_sub(args[1] as u32)
            );
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
}

/// A subtly-wrong contract is REFUTED by proof search, not testing: claim
/// add is commutative-with-sub. The cex space (b != 0) is huge so difftest
/// would also catch it — instead claim something tight: result equals
/// a+b+1 only when b==0 is false... keep it simple: a saturating-looking
/// claim that fails only at the wrap boundary, where random testing with a
/// bounded pre cannot reach.
#[test]
fn wrap_boundary_caught_by_solver_not_luck() {
    // Claim: a + b >= a (unsigned) — false exactly when a+b wraps 2^32.
    // Pre permits the full range, so the only witnesses are huge values;
    // the solver finds them by construction.
    let contract = r#"
(contract eqc/0
  (target (func "add" (param $a i32) (param $b i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (c32 1))
  (post (ge_u result $a))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(ADD_OK), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Counterexample { args, .. } => {
            let wraps = (args[0] as u32).checked_add(args[1] as u32).is_none();
            assert!(wraps, "cex must be a wrap-boundary witness: {args:?}");
        }
        v => panic!("expected wrap counterexample, got {v:?}"),
    }
}

/// if/else control flow is provable: |a - b| via branch.
#[test]
fn if_else_absdiff_proved() {
    let absdiff = r#"
(module
  (memory (export "memory") 1)
  (func (export "absdiff") (param i32 i32) (result i32)
    (if (result i32) (i32.ge_u (local.get 0) (local.get 1))
      (then (i32.sub (local.get 0) (local.get 1)))
      (else (i32.sub (local.get 1) (local.get 0))))))
"#;
    // Spec the branch behaviour directly.
    let contract = r#"
(contract eqc/0
  (target (func "absdiff" (param $a i32) (param $b i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (c32 1))
  (post (ite (ge_u $a $b)
             (eq result (sub $a $b))
             (eq result (sub $b $a))))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(absdiff), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::Proved(_)),
        "expected proved, got {:?}",
        r.verdict
    );
}

/// Functions outside the BMC envelope (memory ops) still flow to difftest.
#[test]
fn memory_function_falls_back_to_difftest() {
    let contract = r#"
(contract eqc/0
  (target (func "store42" (param $p i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (lt_u $p (c32 65536)))
  (post (eq (byte new $p) (c32 42)))
  (frame (range $p (c32 1))))
"#;
    let store = r#"
(module
  (memory (export "memory") 1)
  (func (export "store42") (param i32)
    (i32.store8 (local.get 0) (i32.const 42))))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(store), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::TestedN { .. }),
        "memory fn must fall back to tested-N, got {:?}",
        r.verdict
    );
}

/// AC-1 over the proving path: proved receipts are byte-identical.
#[test]
fn ac1_proved_receipts_deterministic() {
    let c = parse_contract(ADD_CONTRACT).unwrap();
    let a = wasm(ADD_OK);
    let r1 = check(&a, &c, &DiffConfig::default());
    let r2 = check(&a, &c, &DiffConfig::default());
    assert!(matches!(r1.verdict, Verdict::Proved(_)));
    assert_eq!(r1.to_bytes(), r2.to_bytes());
}

/// AC-3 pin: `mint_unsat(` appears exactly once in engine sources (the BMC
/// UNSAT site) and nowhere else in the workspace's non-core crates.
#[test]
fn ac3_single_minting_site() {
    fn count_in_dir(dir: &std::path::Path, needle: &str) -> usize {
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                n += count_in_dir(&p, needle);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&p).unwrap();
                n += src.matches(needle).count();
            }
        }
        n
    }
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let engine_src = crates.join("equiv-engine/src");
    let cli_src = crates.join("equiv-cli/src");
    assert_eq!(
        count_in_dir(&engine_src, "mint_unsat("),
        1,
        "exactly one minting site allowed in equiv-engine"
    );
    assert_eq!(
        count_in_dir(&cli_src, "mint_unsat("),
        0,
        "the CLI must never mint proof evidence"
    );
}

/// Loops are PROVED via bounded unrolling + unwinding assertion: doubling
/// by repeated increment, bound provable from the pre (n <= 16, k = 64).
#[test]
fn loop_proved_with_unwinding_assertion() {
    let twice = r#"
(module
  (memory (export "memory") 1)
  (func (export "twice") (param i32) (result i32)
    (local i32 i32) ;; i, acc
    (local.set 1 (local.get 0))
    (block
      (loop
        (br_if 1 (i32.eqz (local.get 1)))
        (local.set 2 (i32.add (local.get 2) (i32.const 2)))
        (local.set 1 (i32.sub (local.get 1) (i32.const 1)))
        (br 0)))
    (local.get 2)))
"#;
    let contract = r#"
(contract eqc/0
  (target (func "twice" (param $n i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (le_u $n (c32 16)))
  (post (eq result (add $n $n)))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(twice), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Proved(ev) => assert_eq!(ev.unroll_k(), 64, "unroll bound must be recorded"),
        v => panic!("expected proved, got {v:?}"),
    }
}

/// Same loop, but the pre permits iterations beyond the unroll bound:
/// must NOT claim proved — honest fallback to tested-N.
#[test]
fn loop_beyond_bound_falls_back_honestly() {
    let twice = r#"
(module
  (memory (export "memory") 1)
  (func (export "twice") (param i32) (result i32)
    (local i32 i32)
    (local.set 1 (local.get 0))
    (block
      (loop
        (br_if 1 (i32.eqz (local.get 1)))
        (local.set 2 (i32.add (local.get 2) (i32.const 2)))
        (local.set 1 (i32.sub (local.get 1) (i32.const 1)))
        (br 0)))
    (local.get 2)))
"#;
    let contract = r#"
(contract eqc/0
  (target (func "twice" (param $n i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (le_u $n (c32 10000)))
  (post (eq result (add $n $n)))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(twice), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::TestedN { .. }),
        "bound overflow must degrade to tested-N, got {:?}",
        r.verdict
    );
}

/// A buggy loop body is refuted: increments by 3 instead of 2.
#[test]
fn buggy_loop_gets_counterexample() {
    let buggy = r#"
(module
  (memory (export "memory") 1)
  (func (export "twice") (param i32) (result i32)
    (local i32 i32)
    (local.set 1 (local.get 0))
    (block
      (loop
        (br_if 1 (i32.eqz (local.get 1)))
        (local.set 2 (i32.add (local.get 2) (i32.const 3)))
        (local.set 1 (i32.sub (local.get 1) (i32.const 1)))
        (br 0)))
    (local.get 2)))
"#;
    let contract = r#"
(contract eqc/0
  (target (func "twice" (param $n i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (le_u $n (c32 16)))
  (post (eq result (add $n $n)))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(buggy), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Counterexample { args, .. } => {
            assert!(args[0] >= 1, "n=0 is not a witness for the off-by-one: {args:?}")
        }
        v => panic!("expected counterexample, got {v:?}"),
    }
}

/// Early return through an if: clamp to 100.
#[test]
fn early_return_proved() {
    let clamp = r#"
(module
  (memory (export "memory") 1)
  (func (export "clamp") (param i32) (result i32)
    (if (i32.gt_u (local.get 0) (i32.const 100))
      (then (return (i32.const 100))))
    (local.get 0)))
"#;
    let contract = r#"
(contract eqc/0
  (target (func "clamp" (param $a i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (c32 1))
  (post (ite (gt_u $a (c32 100)) (eq result (c32 100)) (eq result $a)))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(clamp), &c, &DiffConfig::default());
    assert!(
        matches!(r.verdict, Verdict::Proved(_)),
        "expected proved, got {:?}",
        r.verdict
    );
}

/// A reachable `unreachable` under the pre is a trap counterexample found
/// by the SOLVER (guard at 12345 — random testing won't hit it).
#[test]
fn conditional_trap_found_by_solver() {
    let trapdoor = r#"
(module
  (memory (export "memory") 1)
  (func (export "f") (param i32) (result i32)
    (if (i32.eq (local.get 0) (i32.const 12345))
      (then unreachable))
    (local.get 0)))
"#;
    let contract = r#"
(contract eqc/0
  (target (func "f" (param $a i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (c32 1))
  (post (eq result $a))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(trapdoor), &c, &DiffConfig::default());
    match &r.verdict {
        Verdict::Counterexample { args, .. } => assert_eq!(args[0], 12345),
        v => panic!("expected the 12345 trapdoor, got {v:?}"),
    }
}

/// AC-6: an unsatisfiable precondition yields unknown(vacuous-pre).
#[test]
fn ac6_vacuous_pre_detected() {
    let contract = r#"
(contract eqc/0
  (target (func "add" (param $a i32) (param $b i32) (result i32)))
  (assume no-imports no-memory-grow no-trap)
  (pre (and (lt_u $a (c32 5)) (gt_u $a (c32 10))))
  (post (eq result (add $a $b)))
  (frame))
"#;
    let c = parse_contract(contract).unwrap();
    let r = check(&wasm(ADD_OK), &c, &DiffConfig::default());
    assert!(
        matches!(
            r.verdict,
            Verdict::Unknown(equiv_core::UnknownReason::VacuousPre)
        ),
        "expected vacuous-pre, got {:?}",
        r.verdict
    );
}
