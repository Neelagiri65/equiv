//! The contract evaluator's bit-vector operators against the WASM
//! interpreter, on boundary operands: same value where WASM yields one, an
//! error exactly where WASM traps.

use equiv_core::ast::{BvOp, Expr, ValType};
use equiv_engine::eval::{eval, CVal, Env, EvalError};

const OPS: [(BvOp, &str); 13] = [
    (BvOp::Add, "add"),
    (BvOp::Sub, "sub"),
    (BvOp::Mul, "mul"),
    (BvOp::DivU, "div_u"),
    (BvOp::DivS, "div_s"),
    (BvOp::RemU, "rem_u"),
    (BvOp::RemS, "rem_s"),
    (BvOp::And, "and"),
    (BvOp::Or, "or"),
    (BvOp::Xor, "xor"),
    (BvOp::Shl, "shl"),
    (BvOp::ShrU, "shr_u"),
    (BvOp::ShrS, "shr_s"),
];

fn edges(bits: u32) -> Vec<u64> {
    let mask = if bits == 64 { u64::MAX } else { 0xffff_ffff };
    let min = 1u64 << (bits - 1);
    let mut v = vec![0, 1, 2, 3, 7, 31, 32, 33, 63, 64, 65, min - 1, min, min + 1, mask, mask - 1];
    v.iter_mut().for_each(|x| *x &= mask);
    v
}

fn contract_side(op: BvOp, bits: u32, a: u64, b: u64) -> Result<u64, EvalError> {
    let ty = if bits == 64 { ValType::I64 } else { ValType::I32 };
    let args = [a, b];
    let types = [ty, ty];
    let env = Env {
        args: &args,
        arg_types: &types,
        result: None,
        result_type: None,
        old_mem: &[],
        new_mem: &[],
        idx: None,
    };
    let e = Expr::Bv(op, Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
    match eval(&e, &env)? {
        CVal::B32(n) => Ok(n as u64),
        CVal::B64(n) => Ok(n),
        other => panic!("unexpected {other:?}"),
    }
}

/// None = the instruction trapped.
fn wasm_side(name: &str, bits: u32, a: u64, b: u64) -> Option<u64> {
    let t = if bits == 64 { "i64" } else { "i32" };
    let src = format!(
        "(module (func (export \"f\") (param {t} {t}) (result {t}) local.get 0 local.get 1 {t}.{name}))"
    );
    let bytes = wat::parse_str(&src).unwrap();
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes[..]).unwrap();
    let mut store = wasmi::Store::new(&engine, ());
    let linker: wasmi::Linker<()> = wasmi::Linker::new(&engine);
    let inst = linker.instantiate(&mut store, &module).unwrap().start(&mut store).unwrap();
    let f = inst.get_func(&store, "f").unwrap();
    let v = |x: u64| if bits == 64 { wasmi::Value::I64(x as i64) } else { wasmi::Value::I32(x as u32 as i32) };
    let mut out = [wasmi::Value::I32(0)];
    f.call(&mut store, &[v(a), v(b)], &mut out).ok()?;
    Some(match out[0] {
        wasmi::Value::I32(n) => n as u32 as u64,
        wasmi::Value::I64(n) => n as u64,
        _ => unreachable!(),
    })
}

#[test]
fn bv_ops_agree_with_wasm_on_boundary_operands() {
    for bits in [32u32, 64] {
        for (op, name) in OPS {
            for &a in &edges(bits) {
                for &b in &edges(bits) {
                    let ours = contract_side(op, bits, a, b);
                    let theirs = wasm_side(name, bits, a, b);
                    match (&ours, theirs) {
                        (Ok(x), Some(y)) if *x == y => {}
                        (Err(_), None) => {}
                        _ => panic!("i{bits}.{name}({a:#x}, {b:#x}): evaluator {ours:?}, wasm {theirs:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn rem_s_of_min_by_minus_one_is_zero() {
    assert_eq!(contract_side(BvOp::RemS, 32, 0x8000_0000, 0xffff_ffff), Ok(0));
    assert_eq!(contract_side(BvOp::RemS, 64, 1 << 63, u64::MAX), Ok(0));
}

#[test]
fn div_s_of_min_by_minus_one_is_an_overflow_not_a_zero_divisor() {
    assert_eq!(contract_side(BvOp::DivS, 32, 0x8000_0000, 0xffff_ffff), Err(EvalError::Overflow));
    assert_eq!(contract_side(BvOp::DivS, 64, 1 << 63, u64::MAX), Err(EvalError::Overflow));
    assert_eq!(contract_side(BvOp::DivS, 32, 5, 0), Err(EvalError::DivByZero));
    assert_eq!(contract_side(BvOp::RemS, 64, 5, 0), Err(EvalError::DivByZero));
}

/// The solver's shifts take the amount modulo the width, as WASM and the
/// evaluator do. A claim that an over-wide shift clears the value must be
/// refuted with a reproduced counterexample; the true claim must hold.
#[test]
fn solver_shifts_take_the_amount_modulo_the_width() {
    use equiv_core::verdict::Verdict;
    for (t, c, bits) in [("i32", "c32", 32u32), ("i64", "c64", 64)] {
        for op in ["shl", "shr_u", "shr_s"] {
            let module = format!(
                "(module (func (export \"f\") (param {t} {t}) (result {t}) local.get 0 local.get 1 {t}.{op}))"
            );
            let wasm = wat::parse_str(&module).unwrap();
            for amount in [bits, bits + 1, 2 * bits - 1] {
                let contract = |post: &str| {
                    let src = format!(
                        "(contract eqc/0 (target (func \"f\" (param $x {t}) (param $n {t}) (result {t}))) \
                         (pre (eq $n ({c} {amount}))) (post {post}) (frame))"
                    );
                    equiv_core::parse_contract(&src).unwrap()
                };
                let cfg = equiv_engine::DiffConfig::default();
                let wrong = equiv_engine::check(&wasm, &contract(&format!("(eq result ({c} 0))")), &cfg).verdict;
                assert!(
                    matches!(wrong, Verdict::Counterexample { trap: false, .. }),
                    "{t}.{op} by {amount}: {wrong:?}"
                );
                let right = format!("(eq result ({op} $x ({c} {})))", amount % bits);
                let ok = equiv_engine::check(&wasm, &contract(&right), &cfg).verdict;
                #[cfg(feature = "uncertified-proofs")]
                assert!(matches!(ok, Verdict::Proved(_)), "{t}.{op} by {amount}: {ok:?}");
                // Without the feature the solver's UNSAT is not reported and
                // sampling cannot hit the one permitted amount.
                #[cfg(not(feature = "uncertified-proofs"))]
                assert!(matches!(ok, Verdict::Unknown(_) | Verdict::TestedN { .. }), "{t}.{op} by {amount}: {ok:?}");
            }
        }
    }
}
