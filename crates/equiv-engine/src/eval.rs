//! Concrete evaluation of eqc/0 expressions against a concrete execution:
//! argument values, result value, and old/new linear-memory snapshots.
//! This is the executable semantics of the contract language — the
//! differential tester runs it per case, and counterexample replay (AC-4)
//! will reuse it verbatim.

use equiv_core::ast::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CVal {
    B32(u32),
    B64(u64),
    Bool(bool),
    /// (base, len) over linear memory.
    Region(u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    TypeMismatch(&'static str),
    DivByZero,
    OutOfBounds,
    ArgMissing(u8),
    NoResult,
    IdxUnbound,
}

pub struct Env<'a> {
    /// Concrete arguments, widened to u64 (i32 args zero-extended).
    pub args: &'a [u64],
    pub arg_types: &'a [ValType],
    /// The call's result, if any (widened to u64).
    pub result: Option<u64>,
    pub result_type: Option<ValType>,
    pub old_mem: &'a [u8],
    pub new_mem: &'a [u8],
    /// Innermost forall_b bound variable.
    pub idx: Option<u32>,
}

fn mem<'a>(env: &'a Env, s: State) -> &'a [u8] {
    match s {
        State::Old => env.old_mem,
        State::New => env.new_mem,
    }
}

fn as_bool(v: CVal) -> Result<bool, EvalError> {
    match v {
        CVal::Bool(b) => Ok(b),
        // Numeric used in boolean position: non-zero is true (the text
        // surface permits e.g. `(post (c32 1))`; the receipt's trivial-post
        // marker is the guard, not a type error).
        CVal::B32(n) => Ok(n != 0),
        CVal::B64(n) => Ok(n != 0),
        CVal::Region(..) => Err(EvalError::TypeMismatch("region in boolean position")),
    }
}

fn as_region(v: CVal) -> Result<(u32, u32), EvalError> {
    match v {
        CVal::Region(b, l) => Ok((b, l)),
        _ => Err(EvalError::TypeMismatch("expected region")),
    }
}

fn as_addr(v: CVal) -> Result<u32, EvalError> {
    match v {
        CVal::B32(n) => Ok(n),
        _ => Err(EvalError::TypeMismatch("address must be bv32")),
    }
}

fn region_in_bounds(base: u32, len: u32, mem_len: usize) -> bool {
    (base as u64 + len as u64) <= mem_len as u64
}

fn region_bytes<'a>(m: &'a [u8], base: u32, len: u32) -> Result<&'a [u8], EvalError> {
    if !region_in_bounds(base, len, m.len()) {
        return Err(EvalError::OutOfBounds);
    }
    Ok(&m[base as usize..(base as usize + len as usize)])
}

fn bv_binop(op: BvOp, a: CVal, b: CVal) -> Result<CVal, EvalError> {
    match (a, b) {
        (CVal::B32(x), CVal::B32(y)) => Ok(CVal::B32(bv32(op, x, y)?)),
        (CVal::B64(x), CVal::B64(y)) => Ok(CVal::B64(bv64(op, x, y)?)),
        _ => Err(EvalError::TypeMismatch("bv op width mismatch")),
    }
}

fn bv32(op: BvOp, x: u32, y: u32) -> Result<u32, EvalError> {
    Ok(match op {
        BvOp::Add => x.wrapping_add(y),
        BvOp::Sub => x.wrapping_sub(y),
        BvOp::Mul => x.wrapping_mul(y),
        BvOp::DivU => x.checked_div(y).ok_or(EvalError::DivByZero)?,
        BvOp::DivS => (x as i32).checked_div(y as i32).ok_or(EvalError::DivByZero)? as u32,
        BvOp::RemU => x.checked_rem(y).ok_or(EvalError::DivByZero)?,
        BvOp::RemS => (x as i32).checked_rem(y as i32).ok_or(EvalError::DivByZero)? as u32,
        BvOp::And => x & y,
        BvOp::Or => x | y,
        BvOp::Xor => x ^ y,
        BvOp::Shl => x.wrapping_shl(y),
        BvOp::ShrU => x.wrapping_shr(y),
        BvOp::ShrS => ((x as i32).wrapping_shr(y)) as u32,
    })
}

fn bv64(op: BvOp, x: u64, y: u64) -> Result<u64, EvalError> {
    Ok(match op {
        BvOp::Add => x.wrapping_add(y),
        BvOp::Sub => x.wrapping_sub(y),
        BvOp::Mul => x.wrapping_mul(y),
        BvOp::DivU => x.checked_div(y).ok_or(EvalError::DivByZero)?,
        BvOp::DivS => (x as i64).checked_div(y as i64).ok_or(EvalError::DivByZero)? as u64,
        BvOp::RemU => x.checked_rem(y).ok_or(EvalError::DivByZero)?,
        BvOp::RemS => (x as i64).checked_rem(y as i64).ok_or(EvalError::DivByZero)? as u64,
        BvOp::And => x & y,
        BvOp::Or => x | y,
        BvOp::Xor => x ^ y,
        BvOp::Shl => x.wrapping_shl(y as u32),
        BvOp::ShrU => x.wrapping_shr(y as u32),
        BvOp::ShrS => ((x as i64).wrapping_shr(y as u32)) as u64,
    })
}

fn cmp(op: CmpOp, a: CVal, b: CVal) -> Result<bool, EvalError> {
    let (xu, yu, xs, ys) = match (a, b) {
        (CVal::B32(x), CVal::B32(y)) => (x as u64, y as u64, x as i32 as i64, y as i32 as i64),
        (CVal::B64(x), CVal::B64(y)) => (x, y, x as i64, y as i64),
        (CVal::Bool(x), CVal::Bool(y)) => {
            return match op {
                CmpOp::Eq => Ok(x == y),
                CmpOp::Ne => Ok(x != y),
                _ => Err(EvalError::TypeMismatch("ordered compare on bool")),
            }
        }
        _ => return Err(EvalError::TypeMismatch("compare width mismatch")),
    };
    Ok(match op {
        CmpOp::Eq => xu == yu,
        CmpOp::Ne => xu != yu,
        CmpOp::LtU => xu < yu,
        CmpOp::LtS => xs < ys,
        CmpOp::LeU => xu <= yu,
        CmpOp::LeS => xs <= ys,
        CmpOp::GtU => xu > yu,
        CmpOp::GtS => xs > ys,
        CmpOp::GeU => xu >= yu,
        CmpOp::GeS => xs >= ys,
    })
}

pub fn eval(e: &Expr, env: &Env) -> Result<CVal, EvalError> {
    Ok(match e {
        Expr::C32(n) => CVal::B32(*n),
        Expr::C64(n) => CVal::B64(*n),
        Expr::Arg(i) => {
            let v = *env.args.get(*i as usize).ok_or(EvalError::ArgMissing(*i))?;
            match env.arg_types.get(*i as usize) {
                Some(ValType::I64) => CVal::B64(v),
                Some(ValType::I32) => CVal::B32(v as u32),
                _ => return Err(EvalError::TypeMismatch("non-integer arg")),
            }
        }
        Expr::Result => {
            let v = env.result.ok_or(EvalError::NoResult)?;
            match env.result_type {
                Some(ValType::I64) => CVal::B64(v),
                Some(ValType::I32) => CVal::B32(v as u32),
                _ => return Err(EvalError::TypeMismatch("non-integer result")),
            }
        }
        Expr::Idx => CVal::B32(env.idx.ok_or(EvalError::IdxUnbound)?),
        Expr::Bv(op, a, b) => bv_binop(*op, eval(a, env)?, eval(b, env)?)?,
        Expr::Zext(a) => match eval(a, env)? {
            CVal::B32(n) => CVal::B64(n as u64),
            _ => return Err(EvalError::TypeMismatch("zext expects bv32")),
        },
        Expr::Wrap(a) => match eval(a, env)? {
            CVal::B64(n) => CVal::B32(n as u32),
            _ => return Err(EvalError::TypeMismatch("wrap expects bv64")),
        },
        Expr::Cmp(op, a, b) => CVal::Bool(cmp(*op, eval(a, env)?, eval(b, env)?)?),
        Expr::Band(a, b) => CVal::Bool(as_bool(eval(a, env)?)? && as_bool(eval(b, env)?)?),
        Expr::Bor(a, b) => CVal::Bool(as_bool(eval(a, env)?)? || as_bool(eval(b, env)?)?),
        Expr::Bnot(a) => CVal::Bool(!as_bool(eval(a, env)?)?),
        Expr::Implies(a, b) => CVal::Bool(!as_bool(eval(a, env)?)? || as_bool(eval(b, env)?)?),
        Expr::Ite(c, a, b) => {
            if as_bool(eval(c, env)?)? {
                eval(a, env)?
            } else {
                eval(b, env)?
            }
        }
        Expr::Byte(s, addr) => {
            let a = as_addr(eval(addr, env)?)?;
            let m = mem(env, *s);
            CVal::B32(*m.get(a as usize).ok_or(EvalError::OutOfBounds)? as u32)
        }
        Expr::Word32(s, addr) => {
            let a = as_addr(eval(addr, env)?)?;
            let b = region_bytes(mem(env, *s), a, 4)?;
            CVal::B32(u32::from_le_bytes(b.try_into().unwrap()))
        }
        Expr::Word64(s, addr) => {
            let a = as_addr(eval(addr, env)?)?;
            let b = region_bytes(mem(env, *s), a, 8)?;
            CVal::B64(u64::from_le_bytes(b.try_into().unwrap()))
        }
        Expr::Range(base, len) => {
            let b = as_addr(eval(base, env)?)?;
            let l = as_addr(eval(len, env)?)?;
            CVal::Region(b, l)
        }
        // In concrete evaluation, readable/writable mean "within linear
        // memory bounds" (old state for readable, new for writable — both
        // snapshots have identical length unless memory grew).
        Expr::Readable(r) => {
            let (b, l) = as_region(eval(r, env)?)?;
            CVal::Bool(region_in_bounds(b, l, env.old_mem.len()))
        }
        Expr::Writable(r) => {
            let (b, l) = as_region(eval(r, env)?)?;
            CVal::Bool(region_in_bounds(b, l, env.old_mem.len()))
        }
        Expr::Disjoint(r1, r2) => {
            let (b1, l1) = as_region(eval(r1, env)?)?;
            let (b2, l2) = as_region(eval(r2, env)?)?;
            let (e1, e2) = (b1 as u64 + l1 as u64, b2 as u64 + l2 as u64);
            CVal::Bool(e1 <= b2 as u64 || e2 <= b1 as u64)
        }
        Expr::MemEq(s1, r1, s2, r2) => {
            let (b1, l1) = as_region(eval(r1, env)?)?;
            let (b2, l2) = as_region(eval(r2, env)?)?;
            if l1 != l2 {
                return Ok(CVal::Bool(false));
            }
            let m1 = region_bytes(mem(env, *s1), b1, l1)?;
            let m2 = region_bytes(mem(env, *s2), b2, l2)?;
            CVal::Bool(m1 == m2)
        }
        Expr::Unchanged(r) => {
            let (b, l) = as_region(eval(r, env)?)?;
            let m1 = region_bytes(env.old_mem, b, l)?;
            let m2 = region_bytes(env.new_mem, b, l)?;
            CVal::Bool(m1 == m2)
        }
        Expr::ForallB(k, n, body) => {
            // Spec §4: the meaning IS the finite conjunction.
            let bound = match eval(n, env)? {
                CVal::B32(v) => v,
                _ => return Err(EvalError::TypeMismatch("forall_b bound must be bv32")),
            };
            let mut inner = Env {
                args: env.args,
                arg_types: env.arg_types,
                result: env.result,
                result_type: env.result_type,
                old_mem: env.old_mem,
                new_mem: env.new_mem,
                idx: None,
            };
            for i in 0..*k {
                if i >= bound {
                    break;
                }
                inner.idx = Some(i);
                if !as_bool(eval(body, &inner)?)? {
                    return Ok(CVal::Bool(false));
                }
            }
            CVal::Bool(true)
        }
    })
}

pub fn eval_bool(e: &Expr, env: &Env) -> Result<bool, EvalError> {
    as_bool(eval(e, env)?)
}
