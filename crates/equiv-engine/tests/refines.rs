//! `refines`: the target and a hash-pinned reference artifact are run on the
//! same inputs and the same memory. Their results, and the bytes inside the
//! contract's frame regions, must agree. Written before the comparison
//! existed: a contract with `refines` was could-not-check whatever was given.
//!
//! Scope of this version: the reference takes the target's own parameters in
//! order; by testing only; everything it cannot compare is could-not-check.

use equiv_core::parse_contract;
use equiv_core::verdict::Verdict;
use equiv_core::UnknownReason;
use equiv_engine::{check_explained, check_with_reference, DiffConfig};

fn wasm(src: &str) -> Vec<u8> {
    wat::parse_str(src).expect("fixture WAT must assemble")
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A scalar contract `f(x) -> i32` that refines export `g` of `reference`.
fn scalar(pre: &str, hash: &str, over: &str) -> String {
    format!(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre {pre}) (post (c32 1)) (frame)
          (refines (artifact sha256:{hash}) "g" {over}))"#
    )
}

fn f(body: &str) -> Vec<u8> {
    wasm(&format!(r#"(module (func (export "f") (param i32) (result i32) {body}))"#))
}

fn g(body: &str) -> Vec<u8> {
    wasm(&format!(r#"(module (func (export "g") (param i32) (result i32) {body}))"#))
}

fn run(target: &[u8], reference: &[u8], contract: &str) -> (Verdict, Option<String>) {
    let c = parse_contract(contract).unwrap();
    let out = check_with_reference(target, Some(reference), &c, &DiffConfig::default());
    // Deterministic: the same check gives the same receipt bytes.
    let again = check_with_reference(target, Some(reference), &c, &DiffConfig::default());
    assert_eq!(out.receipt.to_bytes(), again.receipt.to_bytes());
    (out.receipt.verdict, out.note)
}

fn scalar_run(target: &[u8], reference: &[u8], pre: &str) -> (Verdict, Option<String>) {
    run(target, reference, &scalar(pre, &sha(reference), "(over $x)"))
}

fn unknown(v: &Verdict) -> bool {
    matches!(v, Verdict::Unknown(_))
}

// ---------- pairs that agree ----------

#[test]
fn the_same_sum_written_two_ways_agrees() {
    let (v, note) = scalar_run(&f("local.get 0 i32.const 1 i32.add"), &g("i32.const 1 local.get 0 i32.add"), "(c32 1)");
    // Also under the `uncertified-proofs` feature: a refines contract never
    // takes the symbolic path, so a pass is tested-N, never "proved".
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "{v:?}");
    let note = note.expect("a refines pass says the reference was compared");
    assert!(note.contains("the target and the reference returned the same result on all 64 inputs"), "{note}");
}

#[test]
fn doubling_and_shifting_agree() {
    let (v, _) = scalar_run(&f("local.get 0 i32.const 2 i32.mul"), &g("local.get 0 i32.const 1 i32.shl"), "(c32 1)");
    assert!(matches!(v, Verdict::TestedN { .. }), "{v:?}");
}

#[test]
fn a_difference_the_precondition_excludes_is_not_a_difference() {
    // They differ only above 1000; the contract speaks about x <= 1000.
    let t = f("local.get 0 i32.const 1000 i32.gt_u if (result i32) i32.const 7 else local.get 0 end");
    let (v, _) = scalar_run(&t, &g("local.get 0"), "(le_u $x (c32 1000))");
    assert!(matches!(v, Verdict::TestedN { .. }), "{v:?}");
}

const SUM: &str = r#"(contract eqc/0 (target (func "f" (param $p i32) (param $n i32) (result i32)))
  (pre (and (readable (range $p $n)) (le_u $n (c32 64)))) (post (c32 1)) (frame)
  (refines (artifact sha256:HASH) "g" (over (range $p $n))))"#;

fn byte_sum(name: &str, step_two: bool) -> Vec<u8> {
    // sum of the n bytes at p; the second form adds two bytes per round.
    let body = if step_two {
        r#"(local $i i32) (local $s i32)
           (block $done (loop $l
             (br_if $done (i32.ge_u (i32.add (local.get $i) (i32.const 1)) (local.get 1)))
             (local.set $s (i32.add (local.get $s) (i32.add
               (i32.load8_u (i32.add (local.get 0) (local.get $i)))
               (i32.load8_u (i32.add (local.get 0) (i32.add (local.get $i) (i32.const 1)))))))
             (local.set $i (i32.add (local.get $i) (i32.const 2)))
             (br $l)))
           (if (i32.lt_u (local.get $i) (local.get 1))
             (then (local.set $s (i32.add (local.get $s) (i32.load8_u (i32.add (local.get 0) (local.get $i)))))))
           (local.get $s)"#
    } else {
        r#"(local $i i32) (local $s i32)
           (block $done (loop $l
             (br_if $done (i32.ge_u (local.get $i) (local.get 1)))
             (local.set $s (i32.add (local.get $s) (i32.load8_u (i32.add (local.get 0) (local.get $i)))))
             (local.set $i (i32.add (local.get $i) (i32.const 1)))
             (br $l)))
           (local.get $s)"#
    };
    wasm(&format!(r#"(module (memory (export "memory") 1) (func (export "{name}") (param i32 i32) (result i32) {body}))"#))
}

#[test]
fn a_byte_sum_and_its_unrolled_form_agree_over_memory() {
    let (t, r) = (byte_sum("f", true), byte_sum("g", false));
    let (v, note) = run(&t, &r, &SUM.replace("HASH", &sha(&r)));
    assert!(matches!(v, Verdict::TestedN { .. }), "{v:?}");
    assert!(note.unwrap().contains("returned the same result on all"));
}

// ---------- pairs that differ ----------

#[test]
fn a_different_result_is_a_counterexample_with_both_values() {
    let (v, note) = scalar_run(&f("local.get 0 i32.const 1 i32.add"), &g("local.get 0 i32.const 2 i32.add"), "(c32 1)");
    assert!(matches!(v, Verdict::Counterexample { trap: false, .. }), "{v:?}");
    let note = note.expect("a refines difference says what each side returned");
    assert!(note.contains("reference"), "{note}");
}

#[test]
fn a_difference_at_one_boundary_value_is_found() {
    let t = f("local.get 0 i32.const 0x80000000 i32.eq if (result i32) i32.const 0 else local.get 0 end");
    let (v, _) = scalar_run(&t, &g("local.get 0"), "(c32 1)");
    assert_eq!(v, Verdict::Counterexample { args: vec![0x8000_0000], trap: false });
}

const STORE: &str = r#"(contract eqc/0 (target (func "f" (param $p i32) (result i32)))
  (pre (and (writable (range $p (c32 1))) (lt_u $p (c32 60000)))) (post (c32 1)) (frame (range $p (c32 1)))
  (refines (artifact sha256:HASH) "g" (over $p)))"#;

fn store(name: &str, byte: u32) -> Vec<u8> {
    wasm(&format!(
        r#"(module (memory (export "memory") 1)
             (func (export "{name}") (param i32) (result i32)
               local.get 0 i32.const {byte} i32.store8 i32.const 0))"#
    ))
}

#[test]
fn the_same_result_but_a_different_byte_inside_the_frame_is_a_counterexample() {
    let (t, r) = (store("f", 1), store("g", 2));
    let (v, note) = run(&t, &r, &STORE.replace("HASH", &sha(&r)));
    assert!(matches!(v, Verdict::Counterexample { trap: false, .. }), "{v:?}");
    assert!(note.unwrap().contains("frame"));
}

#[test]
fn the_same_bytes_inside_the_frame_agree() {
    let (t, r) = (store("f", 9), store("g", 9));
    let (v, _) = run(&t, &r, &STORE.replace("HASH", &sha(&r)));
    assert!(matches!(v, Verdict::TestedN { .. }), "{v:?}");
}

// ---------- could not check ----------

#[test]
fn no_reference_supplied_is_could_not_check() {
    let r = g("local.get 0");
    let c = parse_contract(&scalar("(c32 1)", &sha(&r), "(over $x)")).unwrap();
    let out = check_explained(&f("local.get 0"), &c, &DiffConfig::default());
    assert_eq!(out.receipt.verdict, Verdict::Unknown(UnknownReason::UnsupportedFeature));
    assert!(out.note.unwrap().contains("no reference"));
}

#[test]
fn a_reference_with_another_hash_is_could_not_check() {
    let named = g("local.get 0");
    let given = g("local.get 0 i32.const 0 i32.add");
    let (v, note) = run(&f("local.get 0"), &given, &scalar("(c32 1)", &sha(&named), "(over $x)"));
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("nothing was compared"));
}

#[test]
fn a_reference_that_traps_where_the_precondition_holds_is_could_not_check() {
    let r = g("local.get 0 i32.eqz if unreachable end local.get 0");
    let (v, note) = scalar_run(&f("local.get 0"), &r, "(c32 1)");
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("reference trapped"));
}

#[test]
fn a_reference_without_the_named_export_is_could_not_check() {
    let r = wasm(r#"(module (func (export "h") (param i32) (result i32) local.get 0))"#);
    let (v, note) = scalar_run(&f("local.get 0"), &r, "(c32 1)");
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("exports no function named `g`"));
}

#[test]
fn a_reference_with_another_signature_is_could_not_check() {
    let r = wasm(r#"(module (func (export "g") (param i64) (result i32) i32.const 0))"#);
    let (v, note) = scalar_run(&f("i32.const 0"), &r, "(c32 1)");
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("reference"));
}

#[test]
fn a_reference_with_an_import_is_could_not_check() {
    let r = wasm(r#"(module (import "env" "h" (func (param i32) (result i32)))
                     (func (export "g") (param i32) (result i32) local.get 0 call 0))"#);
    let (v, note) = scalar_run(&f("local.get 0"), &r, "(c32 1)");
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("the reference imports 1 function"));
}

#[test]
fn a_reference_whose_memory_cannot_be_read_is_could_not_check() {
    let t = store("f", 1);
    let r = wasm(r#"(module (memory 1) (func (export "g") (param i32) (result i32)
                      local.get 0 i32.const 1 i32.store8 i32.const 0))"#);
    let (v, note) = run(&t, &r, &STORE.replace("HASH", &sha(&r)));
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("does not export it"));
}

#[test]
fn memories_of_different_sizes_are_could_not_check() {
    let t = store("f", 1);
    let r = wasm(r#"(module (memory (export "memory") 2) (func (export "g") (param i32) (result i32)
                      local.get 0 i32.const 1 i32.store8 i32.const 0))"#);
    let (v, note) = run(&t, &r, &STORE.replace("HASH", &sha(&r)));
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("memor"));
}

#[test]
fn an_over_list_that_is_not_the_targets_parameters_in_order_is_could_not_check() {
    let t = wasm(r#"(module (func (export "f") (param i32 i32) (result i32) local.get 0 local.get 1 i32.sub))"#);
    let r = wasm(r#"(module (func (export "g") (param i32 i32) (result i32) local.get 0 local.get 1 i32.sub))"#);
    let c = format!(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (param $y i32) (result i32)))
          (pre (c32 1)) (post (c32 1)) (frame)
          (refines (artifact sha256:{}) "g" (over $y $x)))"#,
        sha(&r)
    );
    let (v, note) = run(&t, &r, &c);
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("over"));
}

#[test]
fn an_assuming_clause_is_could_not_check() {
    let r = g("local.get 0");
    let c = format!(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre (c32 1)) (post (c32 1)) (frame)
          (refines (artifact sha256:{}) "g" (over $x) (assuming (c32 1))))"#,
        sha(&r)
    );
    let (v, note) = run(&f("local.get 0"), &r, &c);
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("assuming"));
}

#[test]
fn too_few_inputs_inside_the_precondition_is_could_not_check() {
    let (v, note) = scalar_run(&f("local.get 0"), &g("local.get 0"), "(eq $x (c32 123456789))");
    assert!(unknown(&v), "{v:?}");
    assert!(note.unwrap().contains("satisfied the precondition"));
}

#[test]
fn a_target_that_breaks_its_own_postcondition_is_still_a_counterexample() {
    let r = g("local.get 0");
    let c = format!(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre (c32 1)) (post (eq result $x)) (frame)
          (refines (artifact sha256:{}) "g" (over $x)))"#,
        sha(&r)
    );
    let (v, _) = run(&f("local.get 0 i32.const 1 i32.add"), &r, &c);
    assert!(matches!(v, Verdict::Counterexample { .. }), "{v:?}");
}

#[test]
fn a_contract_without_refines_is_checked_exactly_as_before() {
    // The reference draws nothing from the generator: the same inputs, the
    // same receipt bytes, whether or not a reference could have been given.
    let c = parse_contract(
        r#"(contract eqc/0 (target (func "f" (param $x i32) (result i32)))
          (pre (c32 1)) (post (eq result (add $x (c32 1)))) (frame))"#,
    )
    .unwrap();
    let t = f("local.get 0 i32.const 1 i32.add");
    let plain = check_explained(&t, &c, &DiffConfig::default());
    let via = check_with_reference(&t, None, &c, &DiffConfig::default());
    assert_eq!(plain.receipt.to_bytes(), via.receipt.to_bytes());
    // Tested-N in a default build; the symbolic path may prove it when the
    // `uncertified-proofs` feature is on. Either way the two entry points agree.
    assert!(matches!(plain.receipt.verdict, Verdict::TestedN { .. } | Verdict::Proved(_)), "{:?}", plain.receipt.verdict);
    assert!(plain.note.is_none(), "{:?}", plain.note);
}
