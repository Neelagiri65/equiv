//! Every clause and declaration in a contract is either evaluated or the
//! check says it could not be: frame against an oddly-named or hidden
//! memory, `refines`, declared signature, trap reporting, sampling coverage.

use equiv_core::parse_contract;
use equiv_core::verdict::Verdict;
use equiv_core::UnknownReason;
use equiv_engine::{check, check_explained, DiffConfig};

fn wasm(src: &str) -> Vec<u8> {
    wat::parse_str(src).expect("fixture WAT must assemble")
}

fn run(module: &str, contract: &str) -> (Verdict, Option<String>) {
    let c = parse_contract(contract).unwrap();
    let out = check_explained(&wasm(module), &c, &DiffConfig::default());
    // The plain entry point must agree, byte for byte.
    assert_eq!(check(&wasm(module), &c, &DiffConfig::default()).to_bytes(), out.receipt.to_bytes());
    (out.receipt.verdict, out.note)
}

const WRITES_NOTHING: &str = r#"(contract eqc/0 (target (func "f" (param $x i32)))
  (pre (c32 1)) (post (c32 1)) (frame))"#;

// ---------- frame ----------

#[test]
fn frame_is_checked_whatever_the_memory_export_is_called() {
    for name in ["memory", "mem", ""] {
        let m = format!(
            r#"(module (memory (export "{name}") 1)
                 (func (export "f") (param i32) i32.const 0 i32.const 255 i32.store8))"#
        );
        let (v, _) = run(&m, WRITES_NOTHING);
        assert!(
            matches!(v, Verdict::Counterexample { trap: false, .. }),
            "export name {name:?}: write outside the frame must be caught, got {v:?}"
        );
    }
}

#[test]
fn frame_with_unexported_memory_is_could_not_check() {
    let m = r#"(module (memory 1)
                 (func (export "f") (param i32) i32.const 0 i32.const 255 i32.store8))"#;
    let (v, note) = run(m, WRITES_NOTHING);
    assert_eq!(v, Verdict::Unknown(UnknownReason::UnsupportedFeature));
    assert!(note.unwrap().contains("frame clause was not evaluated"));
    // Reached through a call, the write is just as invisible.
    let m = r#"(module (memory 1)
                 (func $w i32.const 0 i32.const 255 i32.store8)
                 (func (export "f") (param i32) call $w))"#;
    let (v, _) = run(m, WRITES_NOTHING);
    assert_eq!(v, Verdict::Unknown(UnknownReason::UnsupportedFeature));
}

#[test]
fn pure_function_beside_an_unexported_memory_is_still_tested() {
    // No memory, call or global instruction: it cannot write, so there is
    // nothing the hidden memory could show.
    let m = r#"(module (memory 1)
                 (func (export "f") (param i32) (result i32) local.get 0 i32.const 1 i32.add))"#;
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (eq result (add $x (c32 1)))) (frame))"#;
    let (v, _) = run(m, c);
    assert!(matches!(v, Verdict::TestedN { .. } | Verdict::Proved(_)), "got {v:?}");
}

#[test]
fn permitted_write_through_renamed_memory_passes() {
    let m = r#"(module (memory (export "mem") 1)
                 (func (export "f") (param i32) i32.const 0 i32.const 255 i32.store8))"#;
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32)))
      (pre (c32 1)) (post (eq (byte new (c32 0)) (c32 255))) (frame (range (c32 0) (c32 1))))"#;
    let (v, _) = run(m, c);
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "got {v:?}");
}

// ---------- refines ----------

#[test]
fn refines_is_could_not_check() {
    let m = r#"(module (func (export "f") (param i32) (result i32) local.get 0 i32.const 1 i32.add))"#;
    let c = format!(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre (c32 1)) (post (c32 1)) (frame)
          (refines (artifact sha256:{}) "g" (over $x)))"#,
        "ab".repeat(32)
    );
    let (v, note) = run(m, &c);
    assert_eq!(v, Verdict::Unknown(UnknownReason::UnsupportedFeature));
    assert!(note.unwrap().contains("refines is not evaluated by this version"));
}

// ---------- signature ----------

fn mismatch(module: &str, contract: &str, expect_in_note: &str) {
    let (v, note) = run(module, contract);
    assert_eq!(v, Verdict::Unknown(UnknownReason::IllFormedExpr), "{contract}");
    let note = note.unwrap();
    assert!(note.contains(expect_in_note), "note was: {note}");
}

#[test]
fn declared_result_type_must_match_the_export() {
    // Returns 2^32 as an i64; read as an i32 it would be 0.
    mismatch(
        r#"(module (func (export "f") (param i32) (result i64) i64.const 0x100000000))"#,
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre (c32 1)) (post (eq result (c32 0))) (frame))"#,
        "exports it as (param i32) (result i64)",
    );
}

#[test]
fn declared_param_types_and_counts_must_match_the_export() {
    let inc = r#"(module (func (export "f") (param i32) (result i32) local.get 0 i32.const 1 i32.add))"#;
    // Was reported as a trap counterexample.
    mismatch(
        inc,
        r#"(contract eqc/0 (target (func "f" (param $x i64) (result i32))) (pre (c32 1)) (post (c32 1)) (frame))"#,
        "declares `f` as (param i64) (result i32)",
    );
    mismatch(
        inc,
        r#"(contract eqc/0 (target (func "f" (param $x i32) (param $y i32) (result i32))) (pre (c32 1)) (post (c32 1)) (frame))"#,
        "exports it as (param i32) (result i32)",
    );
    mismatch(
        inc,
        r#"(contract eqc/0 (target (func "f" (param $x i32))) (pre (c32 1)) (post (c32 1)) (frame))"#,
        "(param i32) (result)",
    );
}

#[test]
fn signature_of_a_reexported_import_is_compared_too() {
    let m = r#"(module (import "env" "g" (func $g (param i64) (result i64))) (export "f" (func $g)))"#;
    mismatch(
        m,
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32))) (pre (c32 1)) (post (c32 1)) (frame))"#,
        "exports it as (param i64) (result i64)",
    );
}

#[test]
fn float_signature_is_could_not_check_not_a_trap() {
    let m = r#"(module (func (export "f") (param f32) (result f32) local.get 0))"#;
    let c = r#"(contract eqc/0 (target (func "f" (param $x f32) (result f32))) (pre (c32 1)) (post (c32 1)) (frame))"#;
    let (v, note) = run(m, c);
    assert_eq!(v, Verdict::Unknown(UnknownReason::UnsupportedFeature));
    assert!(note.unwrap().contains("only i32 and i64"));
}

// ---------- trap flag ----------

const TRAP_AT_5: &str = r#"(module (func (export "f") (param i32) (result i32)
    local.get 0 i32.const 5 i32.eq if unreachable end local.get 0))"#;

#[test]
fn solver_found_trap_is_flagged_as_a_trap() {
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (c32 1)) (frame))"#;
    let (v, _) = run(TRAP_AT_5, c);
    assert_eq!(v, Verdict::Counterexample { args: vec![5], trap: true });
}

#[test]
fn solver_found_wrong_value_is_not_flagged_as_a_trap() {
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (ne $x (c32 5))) (post (eq result (add $x (c32 1)))) (frame))"#;
    let (v, _) = run(TRAP_AT_5, c);
    assert!(matches!(v, Verdict::Counterexample { trap: false, .. }), "got {v:?}");
}

#[test]
fn sampled_trap_and_sampled_wrong_value_are_told_apart() {
    // Memory access keeps both off the symbolic path.
    let traps = r#"(module (memory (export "memory") 1)
        (func (export "f") (param i32) (result i32) i32.const 0 i32.load drop unreachable))"#;
    let wrong = r#"(module (memory (export "memory") 1)
        (func (export "f") (param i32) (result i32) i32.const 0 i32.load drop i32.const 7))"#;
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (eq result (c32 8))) (frame))"#;
    assert!(matches!(run(traps, c).0, Verdict::Counterexample { trap: true, .. }));
    assert!(matches!(run(wrong, c).0, Verdict::Counterexample { trap: false, .. }));
}

// ---------- sampling ----------

/// Echoes through memory so the check goes to the differential tester.
const VIA_MEMORY: &str = r#"(module (memory (export "m") 1)
    (func (export "f") (param i32) (result i32)
      i32.const 0 local.get 0 i32.store i32.const 0 i32.load))"#;

fn diff(module: &str, contract: &str, cfg: &DiffConfig) -> (Verdict, Option<String>) {
    equiv_engine::difftest::difftest_explained(&wasm(module), &parse_contract(contract).unwrap(), cfg)
}

#[test]
fn every_boundary_value_is_tried() {
    // Each contract is wrong at exactly one boundary value, which random
    // sampling would essentially never produce.
    for bad in ["0", "1", "0xffffffff", "0x80000000", "0x7fffffff"] {
        let c = format!(
            r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
              (pre (c32 1)) (post (and (eq result $x) (ne $x (c32 {bad})))) (frame (range (c32 0) (c32 4))))"#
        );
        let (v, _) = diff(VIA_MEMORY, &c, &DiffConfig::default());
        let want = u64::from_str_radix(bad.trim_start_matches("0x"), if bad.starts_with("0x") { 16 } else { 10 }).unwrap();
        assert_eq!(v, Verdict::Counterexample { args: vec![want], trap: false }, "boundary {bad}");
    }
    let m64 = r#"(module (memory (export "m") 1)
        (func (export "f") (param i64) (result i64)
          i32.const 0 local.get 0 i64.store i32.const 0 i64.load))"#;
    for bad in ["0", "1", "0xffffffffffffffff", "0x8000000000000000", "0x7fffffffffffffff"] {
        let c = format!(
            r#"(contract eqc/0 (target (func "f" (param $x i64) (result i64)))
              (pre (c32 1)) (post (and (eq result $x) (ne $x (c64 {bad})))) (frame (range (c32 0) (c32 8))))"#
        );
        let (v, _) = diff(m64, &c, &DiffConfig::default());
        assert!(matches!(v, Verdict::Counterexample { trap: false, .. }), "boundary {bad}: {v:?}");
    }
}

#[test]
fn boundary_pairs_are_tried_for_two_parameters() {
    let m = r#"(module (memory (export "m") 1)
        (func (export "f") (param i32 i32) (result i32)
          i32.const 0 local.get 0 i32.store i32.const 0 i32.load))"#;
    // Wrong only at (MIN, -1).
    let c = r#"(contract eqc/0 (target (func "f" (param $a i32) (param $b i32) (result i32)))
      (pre (c32 1))
      (post (not (and (eq $a (c32 0x80000000)) (eq $b (c32 0xffffffff)))))
      (frame (range (c32 0) (c32 4))))"#;
    let (v, _) = diff(m, c, &DiffConfig::default());
    assert_eq!(v, Verdict::Counterexample { args: vec![0x8000_0000, 0xffff_ffff], trap: false });
}

#[test]
fn inputs_without_a_memory_are_not_mostly_zero() {
    // No memory at all. Half of all samples used to be 0.
    let m = r#"(module (func $id (param i32) (result i32) local.get 0)
                 (func (export "f") (param i32) (result i32) local.get 0 call $id))"#;
    // Accept only inputs above 255: zero, the small class and most
    // boundaries are all excluded, so reaching 64 cases needs a real spread
    // of full-width values.
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (gt_u $x (c32 255))) (post (eq result $x)) (frame))"#;
    let cfg = DiffConfig { max_attempts: 200, ..DiffConfig::default() };
    let (v, note) = diff(m, c, &cfg);
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "got {v:?} {note:?}");
}

#[test]
fn aligned_and_odd_offsets_are_both_sampled() {
    // The class and the value used to share one draw, which pinned the low
    // two bits of every in-memory offset away from 0.
    for (rem, what) in [(0u32, "aligned"), (1, "odd")] {
        let c = format!(
            r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
              (pre (and (gt_u $x (c32 255)) (lt_u $x (c32 65536)) (eq (bvand $x (c32 3)) (c32 {rem}))))
              (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#
        );
        let (v, note) = diff(VIA_MEMORY, &c, &DiffConfig::default());
        assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "{what}: {v:?} {note:?}");
    }
}

#[test]
fn seed_zero_still_varies_its_inputs() {
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (gt_u $x (c32 255))) (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#;
    let cfg = DiffConfig { seed: 0, ..DiffConfig::default() };
    let (v, note) = diff(VIA_MEMORY, c, &cfg);
    assert_eq!(v, Verdict::TestedN { n_cases: 64, seed: 0 }, "{note:?}");
}

#[test]
fn too_few_inputs_passing_the_precondition_is_could_not_check() {
    // About one sample in a thousand passes; a handful would have been
    // reported as tested-N with exit 0.
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (and (gt_u $x (c32 255)) (eq (bvand $x (c32 0x3ff)) (c32 0x155))))
      (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#;
    let (v, note) = diff(VIA_MEMORY, c, &DiffConfig::default());
    assert_eq!(v, Verdict::Unknown(UnknownReason::BudgetExceeded));
    assert_eq!(v.exit_code(), 2);
    let note = note.unwrap();
    assert!(note.contains("of the 64 requested inputs satisfied the precondition in 3200 attempts"), "{note}");
    assert!(!note.starts_with("only 0 "), "the test wants a few passing inputs, not none: {note}");
}

#[test]
fn tested_n_always_reports_the_requested_count() {
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#;
    // Asking for no cases is not a pass.
    let none = DiffConfig { n_cases: 0, ..DiffConfig::default() };
    assert_eq!(diff(VIA_MEMORY, c, &none).0, Verdict::Unknown(UnknownReason::BudgetExceeded));
    for n in [1u64, 4, 8, 64, 200] {
        let cfg = DiffConfig { n_cases: n, max_attempts: n * 50, ..DiffConfig::default() };
        let (v, _) = diff(VIA_MEMORY, c, &cfg);
        assert!(matches!(v, Verdict::TestedN { n_cases, .. } if n_cases == n), "n={n}: {v:?}");
    }
}

#[test]
fn a_pass_that_skipped_inputs_says_how_many() {
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (gt_u $x (c32 255))) (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#;
    let (v, note) = diff(VIA_MEMORY, c, &DiffConfig::default());
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }));
    assert!(note.unwrap().contains("did not satisfy the precondition"));
    // Nothing skipped, nothing to say.
    let all = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (eq result $x)) (frame (range (c32 0) (c32 4))))"#;
    assert_eq!(diff(VIA_MEMORY, all, &DiffConfig::default()).1, None);
}

#[test]
fn the_sampling_path_called_directly_refuses_a_wrong_signature() {
    let m = r#"(module (memory (export "memory") 1)
        (func (export "f") (param i32) (result i64) i64.const 0x100000000))"#;
    let c = r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
      (pre (c32 1)) (post (eq result (c32 0))) (frame))"#;
    let (v, note) = diff(m, c, &DiffConfig::default());
    assert_eq!(v, Verdict::Unknown(UnknownReason::IllFormedExpr));
    assert!(note.unwrap().contains("(result i64)"));
}

#[test]
fn notes_never_overclaim() {
    let m = r#"(module (memory 1) (func (export "f") (param i32) i32.const 0 i32.const 255 i32.store8))"#;
    let (_, note) = run(m, WRITES_NOTHING);
    let note = note.unwrap().to_lowercase();
    for word in ["verified", "safe", "proved"] {
        assert!(!note.contains(word), "{note}");
    }
}
