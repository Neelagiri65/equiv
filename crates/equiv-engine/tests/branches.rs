//! Branches in the symbolic executor follow WASM: a taken branch keeps the
//! label's result values and drops everything else pushed since the block
//! was entered. Each case is run under both feature settings; whichever
//! path decides, the verdict must match what the function really returns.

use equiv_core::parse_contract;
use equiv_core::verdict::Verdict;
use equiv_engine::{check, DiffConfig};

fn verdict(body: &str, sig: &str, post: &str) -> Verdict {
    let module = format!(r#"(module (func (export "f") {sig} {body}))"#);
    let params = if sig.contains("param i32 i32") {
        "(param $x i32) (param $y i32)"
    } else {
        "(param $x i32)"
    };
    let contract = format!(
        r#"(contract eqc/0 (target (func "f" {params} (result i32))) (pre (c32 1)) (post {post}) (frame))"#
    );
    let wasm = wat::parse_str(&module).expect("fixture WAT must assemble");
    check(&wasm, &parse_contract(&contract).unwrap(), &DiffConfig::default()).verdict
}

fn holds(body: &str, sig: &str, post: &str) {
    let v = verdict(body, sig, post);
    #[cfg(feature = "uncertified-proofs")]
    assert!(matches!(v, Verdict::Proved(_)), "{body}: expected the symbolic path to decide, got {v:?}");
    #[cfg(not(feature = "uncertified-proofs"))]
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "{body}: got {v:?}");
}

fn refuted(body: &str, sig: &str, post: &str) {
    let v = verdict(body, sig, post);
    assert!(matches!(v, Verdict::Counterexample { trap: false, .. }), "{body}: got {v:?}");
}

const ONE: &str = "(param i32) (result i32)";

#[test]
fn br_drops_values_below_the_label_result() {
    // Really returns 10 + 2 = 12. Without unwinding the executor saw 1 + 2.
    let body = "i32.const 10 (block (result i32) i32.const 1 i32.const 2 br 0) i32.add";
    holds(body, ONE, "(eq result (c32 12))");
    refuted(body, ONE, "(eq result (c32 3))");
}

#[test]
fn br_out_of_two_blocks_unwinds_both() {
    let body = "i32.const 100
                (block (result i32)
                  i32.const 7
                  (block (result i32) i32.const 8 i32.const 9 local.get 0 br 1)
                  i32.add)
                i32.add";
    holds(body, ONE, "(eq result (add $x (c32 100)))");
    refuted(body, ONE, "(eq result (add $x (c32 9)))");
}

#[test]
fn br_if_taken_unwinds_and_untaken_keeps_the_stack() {
    // x != 0: branch with 20, leaving 5 behind -> 1 + 20.
    // x == 0: fall through, drop 20, then 5 + 3 -> 1 + 8.
    let body = "i32.const 1
                (block (result i32)
                  i32.const 5
                  i32.const 20
                  local.get 0
                  br_if 0
                  drop
                  i32.const 3
                  i32.add)
                i32.add";
    holds(body, ONE, "(eq result (ite (ne $x (c32 0)) (c32 21) (c32 9)))");
    refuted(body, ONE, "(eq result (c32 9))");
    refuted(body, ONE, "(eq result (c32 21))");
}

#[test]
fn br_to_an_empty_label_drops_everything_pushed_inside() {
    let body = "i32.const 4 (block i32.const 1 i32.const 2 br 0) i32.const 6 i32.add";
    holds(body, ONE, "(eq result (c32 10))");
    refuted(body, ONE, "(eq result (c32 8))");
}

#[test]
fn br_out_of_an_if_arm_unwinds_to_the_if() {
    let body = "i32.const 50
                local.get 0
                (if (result i32)
                  (then i32.const 1 i32.const 2 br 0)
                  (else i32.const 3))
                i32.add";
    holds(body, ONE, "(eq result (ite (ne $x (c32 0)) (c32 52) (c32 53)))");
    refuted(body, ONE, "(eq result (ite (ne $x (c32 0)) (c32 3) (c32 53)))");
}

#[test]
fn return_keeps_only_the_result() {
    let body = "i32.const 1 i32.const 2
                (block local.get 0 i32.eqz br_if 0 i32.const 9 i32.const 40 return)
                i32.add";
    holds(body, ONE, "(eq result (ite (eq $x (c32 0)) (c32 3) (c32 40)))");
    refuted(body, ONE, "(eq result (c32 3))");
}

#[test]
fn branch_back_to_a_loop_header_drops_the_body_stack() {
    // Counts x & 3 down to zero, leaving junk on the stack at each back edge.
    let body = "(local i32)
                local.get 0 i32.const 3 i32.and local.set 1
                (block
                  (loop
                    local.get 1 i32.eqz br_if 1
                    local.get 1 i32.const 1 i32.sub local.set 1
                    i32.const 77 i32.const 78
                    br 0))
                local.get 1";
    holds(body, ONE, "(eq result (c32 0))");
    refuted(body, ONE, "(eq result (c32 77))");
}

#[test]
fn br_table_is_outside_the_symbolic_envelope_and_still_checked() {
    // Not modelled symbolically; the interpreter decides.
    let body = "i32.const 10
                (block (result i32)
                  i32.const 1 i32.const 2 local.get 0 br_table 0 0)
                i32.add";
    let v = verdict(body, ONE, "(eq result (c32 12))");
    assert!(matches!(v, Verdict::TestedN { n_cases: 64, .. }), "got {v:?}");
    refuted(body, ONE, "(eq result (c32 3))");
}
