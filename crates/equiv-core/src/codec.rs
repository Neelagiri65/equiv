//! Contract <-> canonical bytes. Encoding is total and deterministic; decoding
//! is strict: unknown map keys, unknown opcodes, unknown flag/type codes, and
//! any non-zero feature bit are rejected (spec rule: unknown => reject).

use crate::ast::*;
use crate::cbor::{self, CborError, Value};

// Top-level map keys (spec §2).
const K_VERSION: u64 = 0;
const K_TARGET: u64 = 1;
const K_ASSUME: u64 = 2;
const K_PRE: u64 = 3;
const K_POST: u64 = 4;
const K_FRAME: u64 = 5;
const K_REFINES: u64 = 6;
const K_FEATURES: u64 = 7;

// Expression opcodes (spec §3). The numbering is part of the format.
mod op {
    pub const C32: u64 = 0;
    pub const C64: u64 = 1;
    pub const ARG: u64 = 2;
    pub const RESULT: u64 = 3;
    pub const IDX: u64 = 4;
    pub const ADD: u64 = 10;
    pub const SUB: u64 = 11;
    pub const MUL: u64 = 12;
    pub const DIV_U: u64 = 13;
    pub const DIV_S: u64 = 14;
    pub const REM_U: u64 = 15;
    pub const REM_S: u64 = 16;
    pub const AND: u64 = 17;
    pub const OR: u64 = 18;
    pub const XOR: u64 = 19;
    pub const SHL: u64 = 20;
    pub const SHR_U: u64 = 21;
    pub const SHR_S: u64 = 22;
    pub const ZEXT: u64 = 25;
    pub const WRAP: u64 = 26;
    pub const EQ: u64 = 30;
    pub const NE: u64 = 31;
    pub const LT_U: u64 = 32;
    pub const LT_S: u64 = 33;
    pub const LE_U: u64 = 34;
    pub const LE_S: u64 = 35;
    pub const GT_U: u64 = 36;
    pub const GT_S: u64 = 37;
    pub const GE_U: u64 = 38;
    pub const GE_S: u64 = 39;
    pub const BAND: u64 = 45;
    pub const BOR: u64 = 46;
    pub const BNOT: u64 = 47;
    pub const IMPLIES: u64 = 48;
    pub const ITE: u64 = 49;
    pub const BYTE: u64 = 55;
    pub const WORD32: u64 = 56;
    pub const WORD64: u64 = 57;
    pub const RANGE: u64 = 60;
    pub const READABLE: u64 = 61;
    pub const WRITABLE: u64 = 62;
    pub const DISJOINT: u64 = 63;
    pub const MEM_EQ: u64 = 64;
    pub const UNCHANGED: u64 = 65;
    pub const FORALL_B: u64 = 70;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Cbor(CborError),
    UnknownTopLevelKey(u64),
    MissingField(u64),
    UnknownFeatureBits(u64),
    UnknownOpcode(u64),
    UnknownValType(u64),
    UnknownAssumeFlag(u64),
    UnknownState(u64),
    BadShape(&'static str),
    UnsupportedVersion(u64),
    Validate(ValidateError),
}

impl From<CborError> for DecodeError {
    fn from(e: CborError) -> Self {
        DecodeError::Cbor(e)
    }
}

// ---------- encode ----------

fn bv_op_code(o: BvOp) -> u64 {
    match o {
        BvOp::Add => op::ADD,
        BvOp::Sub => op::SUB,
        BvOp::Mul => op::MUL,
        BvOp::DivU => op::DIV_U,
        BvOp::DivS => op::DIV_S,
        BvOp::RemU => op::REM_U,
        BvOp::RemS => op::REM_S,
        BvOp::And => op::AND,
        BvOp::Or => op::OR,
        BvOp::Xor => op::XOR,
        BvOp::Shl => op::SHL,
        BvOp::ShrU => op::SHR_U,
        BvOp::ShrS => op::SHR_S,
    }
}

fn cmp_op_code(o: CmpOp) -> u64 {
    match o {
        CmpOp::Eq => op::EQ,
        CmpOp::Ne => op::NE,
        CmpOp::LtU => op::LT_U,
        CmpOp::LtS => op::LT_S,
        CmpOp::LeU => op::LE_U,
        CmpOp::LeS => op::LE_S,
        CmpOp::GtU => op::GT_U,
        CmpOp::GtS => op::GT_S,
        CmpOp::GeU => op::GE_U,
        CmpOp::GeS => op::GE_S,
    }
}

fn state_code(s: State) -> u64 {
    match s {
        State::Old => 0,
        State::New => 1,
    }
}

fn expr_value(e: &Expr) -> Value {
    let arr = |items: Vec<Value>| Value::Array(items);
    let u = Value::Uint;
    match e {
        Expr::C32(k) => arr(vec![u(op::C32), u(*k as u64)]),
        Expr::C64(k) => arr(vec![u(op::C64), u(*k)]),
        Expr::Arg(n) => arr(vec![u(op::ARG), u(*n as u64)]),
        Expr::Result => arr(vec![u(op::RESULT)]),
        Expr::Idx => arr(vec![u(op::IDX)]),
        Expr::Bv(o, a, b) => arr(vec![u(bv_op_code(*o)), expr_value(a), expr_value(b)]),
        Expr::Zext(a) => arr(vec![u(op::ZEXT), expr_value(a)]),
        Expr::Wrap(a) => arr(vec![u(op::WRAP), expr_value(a)]),
        Expr::Cmp(o, a, b) => arr(vec![u(cmp_op_code(*o)), expr_value(a), expr_value(b)]),
        Expr::Band(a, b) => arr(vec![u(op::BAND), expr_value(a), expr_value(b)]),
        Expr::Bor(a, b) => arr(vec![u(op::BOR), expr_value(a), expr_value(b)]),
        Expr::Bnot(a) => arr(vec![u(op::BNOT), expr_value(a)]),
        Expr::Implies(a, b) => arr(vec![u(op::IMPLIES), expr_value(a), expr_value(b)]),
        Expr::Ite(c, a, b) => arr(vec![u(op::ITE), expr_value(c), expr_value(a), expr_value(b)]),
        Expr::Byte(s, a) => arr(vec![u(op::BYTE), u(state_code(*s)), expr_value(a)]),
        Expr::Word32(s, a) => arr(vec![u(op::WORD32), u(state_code(*s)), expr_value(a)]),
        Expr::Word64(s, a) => arr(vec![u(op::WORD64), u(state_code(*s)), expr_value(a)]),
        Expr::Range(a, b) => arr(vec![u(op::RANGE), expr_value(a), expr_value(b)]),
        Expr::Readable(a) => arr(vec![u(op::READABLE), expr_value(a)]),
        Expr::Writable(a) => arr(vec![u(op::WRITABLE), expr_value(a)]),
        Expr::Disjoint(a, b) => arr(vec![u(op::DISJOINT), expr_value(a), expr_value(b)]),
        Expr::MemEq(s1, a, s2, b) => arr(vec![
            u(op::MEM_EQ),
            u(state_code(*s1)),
            expr_value(a),
            u(state_code(*s2)),
            expr_value(b),
        ]),
        Expr::Unchanged(a) => arr(vec![u(op::UNCHANGED), expr_value(a)]),
        Expr::ForallB(k, n, body) => arr(vec![
            u(op::FORALL_B),
            u(*k as u64),
            expr_value(n),
            expr_value(body),
        ]),
    }
}

fn target_value(t: &Target) -> Value {
    Value::Array(vec![
        Value::Text(t.name.clone()),
        Value::Array(t.params.iter().map(|p| Value::Uint(p.code())).collect()),
        Value::Array(t.results.iter().map(|r| Value::Uint(r.code())).collect()),
    ])
}

fn refines_value(r: &Refines) -> Value {
    let mut items = vec![
        Value::Bytes(r.artifact_sha256.to_vec()),
        Value::Text(r.func.clone()),
        Value::Array(r.over.iter().map(expr_value).collect()),
    ];
    if let Some(a) = &r.assuming {
        items.push(expr_value(a));
    }
    Value::Array(items)
}

/// Canonical bytes of a contract. The SHA-256 of this is the contract's
/// identity.
pub fn encode_contract(c: &Contract) -> Vec<u8> {
    let mut entries: Vec<(u64, Value)> = Vec::new();
    entries.push((K_VERSION, Value::Uint(c.version as u64)));
    entries.push((K_TARGET, target_value(&c.target)));
    if !c.assume.is_empty() {
        let mut flags: Vec<u64> = c.assume.iter().map(|f| f.code()).collect();
        flags.sort_unstable();
        flags.dedup();
        entries.push((
            K_ASSUME,
            Value::Array(flags.into_iter().map(Value::Uint).collect()),
        ));
    }
    entries.push((K_PRE, expr_value(&c.pre)));
    entries.push((K_POST, expr_value(&c.post)));
    entries.push((K_FRAME, Value::Array(c.frame.iter().map(expr_value).collect())));
    if let Some(r) = &c.refines {
        entries.push((K_REFINES, refines_value(r)));
    }
    if c.features != 0 {
        entries.push((K_FEATURES, Value::Uint(c.features)));
    }
    cbor::to_bytes(&Value::Map(entries))
}

pub fn contract_sha256(c: &Contract) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(encode_contract(c));
    h.finalize().into()
}

// ---------- decode ----------

fn as_array<'a>(v: &'a Value, what: &'static str) -> Result<&'a [Value], DecodeError> {
    match v {
        Value::Array(items) => Ok(items),
        _ => Err(DecodeError::BadShape(what)),
    }
}

fn as_uint(v: &Value, what: &'static str) -> Result<u64, DecodeError> {
    match v {
        Value::Uint(n) => Ok(*n),
        _ => Err(DecodeError::BadShape(what)),
    }
}

fn decode_state(v: &Value) -> Result<State, DecodeError> {
    match as_uint(v, "state")? {
        0 => Ok(State::Old),
        1 => Ok(State::New),
        n => Err(DecodeError::UnknownState(n)),
    }
}

fn bv_op_from(code: u64) -> Option<BvOp> {
    Some(match code {
        op::ADD => BvOp::Add,
        op::SUB => BvOp::Sub,
        op::MUL => BvOp::Mul,
        op::DIV_U => BvOp::DivU,
        op::DIV_S => BvOp::DivS,
        op::REM_U => BvOp::RemU,
        op::REM_S => BvOp::RemS,
        op::AND => BvOp::And,
        op::OR => BvOp::Or,
        op::XOR => BvOp::Xor,
        op::SHL => BvOp::Shl,
        op::SHR_U => BvOp::ShrU,
        op::SHR_S => BvOp::ShrS,
        _ => return None,
    })
}

fn cmp_op_from(code: u64) -> Option<CmpOp> {
    Some(match code {
        op::EQ => CmpOp::Eq,
        op::NE => CmpOp::Ne,
        op::LT_U => CmpOp::LtU,
        op::LT_S => CmpOp::LtS,
        op::LE_U => CmpOp::LeU,
        op::LE_S => CmpOp::LeS,
        op::GT_U => CmpOp::GtU,
        op::GT_S => CmpOp::GtS,
        op::GE_U => CmpOp::GeU,
        op::GE_S => CmpOp::GeS,
        _ => return None,
    })
}

fn decode_expr(v: &Value) -> Result<Expr, DecodeError> {
    let items = as_array(v, "expr")?;
    if items.is_empty() {
        return Err(DecodeError::BadShape("empty expr"));
    }
    let code = as_uint(&items[0], "opcode")?;
    let args = &items[1..];
    let need = |n: usize| -> Result<(), DecodeError> {
        if args.len() == n {
            Ok(())
        } else {
            Err(DecodeError::BadShape("arity"))
        }
    };
    if let Some(o) = bv_op_from(code) {
        need(2)?;
        return Ok(Expr::Bv(o, decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into()));
    }
    if let Some(o) = cmp_op_from(code) {
        need(2)?;
        return Ok(Expr::Cmp(o, decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into()));
    }
    Ok(match code {
        op::C32 => {
            need(1)?;
            let n = as_uint(&args[0], "c32")?;
            let n32 = u32::try_from(n).map_err(|_| DecodeError::BadShape("c32 range"))?;
            Expr::C32(n32)
        }
        op::C64 => {
            need(1)?;
            Expr::C64(as_uint(&args[0], "c64")?)
        }
        op::ARG => {
            need(1)?;
            let n = as_uint(&args[0], "arg")?;
            Expr::Arg(u8::try_from(n).map_err(|_| DecodeError::BadShape("arg range"))?)
        }
        op::RESULT => {
            need(0)?;
            Expr::Result
        }
        op::IDX => {
            need(0)?;
            Expr::Idx
        }
        op::ZEXT => {
            need(1)?;
            Expr::Zext(decode_expr(&args[0])?.into())
        }
        op::WRAP => {
            need(1)?;
            Expr::Wrap(decode_expr(&args[0])?.into())
        }
        op::BAND => {
            need(2)?;
            Expr::Band(decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into())
        }
        op::BOR => {
            need(2)?;
            Expr::Bor(decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into())
        }
        op::BNOT => {
            need(1)?;
            Expr::Bnot(decode_expr(&args[0])?.into())
        }
        op::IMPLIES => {
            need(2)?;
            Expr::Implies(decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into())
        }
        op::ITE => {
            need(3)?;
            Expr::Ite(
                decode_expr(&args[0])?.into(),
                decode_expr(&args[1])?.into(),
                decode_expr(&args[2])?.into(),
            )
        }
        op::BYTE => {
            need(2)?;
            Expr::Byte(decode_state(&args[0])?, decode_expr(&args[1])?.into())
        }
        op::WORD32 => {
            need(2)?;
            Expr::Word32(decode_state(&args[0])?, decode_expr(&args[1])?.into())
        }
        op::WORD64 => {
            need(2)?;
            Expr::Word64(decode_state(&args[0])?, decode_expr(&args[1])?.into())
        }
        op::RANGE => {
            need(2)?;
            Expr::Range(decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into())
        }
        op::READABLE => {
            need(1)?;
            Expr::Readable(decode_expr(&args[0])?.into())
        }
        op::WRITABLE => {
            need(1)?;
            Expr::Writable(decode_expr(&args[0])?.into())
        }
        op::DISJOINT => {
            need(2)?;
            Expr::Disjoint(decode_expr(&args[0])?.into(), decode_expr(&args[1])?.into())
        }
        op::MEM_EQ => {
            need(4)?;
            Expr::MemEq(
                decode_state(&args[0])?,
                decode_expr(&args[1])?.into(),
                decode_state(&args[2])?,
                decode_expr(&args[3])?.into(),
            )
        }
        op::UNCHANGED => {
            need(1)?;
            Expr::Unchanged(decode_expr(&args[0])?.into())
        }
        op::FORALL_B => {
            need(3)?;
            let k = as_uint(&args[0], "forall_b k")?;
            let k = u32::try_from(k).map_err(|_| DecodeError::BadShape("forall_b k range"))?;
            Expr::ForallB(k, decode_expr(&args[1])?.into(), decode_expr(&args[2])?.into())
        }
        other => return Err(DecodeError::UnknownOpcode(other)),
    })
}

fn decode_target(v: &Value) -> Result<Target, DecodeError> {
    let items = as_array(v, "target")?;
    if items.len() != 3 {
        return Err(DecodeError::BadShape("target arity"));
    }
    let name = match &items[0] {
        Value::Text(s) => s.clone(),
        _ => return Err(DecodeError::BadShape("target name")),
    };
    let types = |v: &Value| -> Result<Vec<ValType>, DecodeError> {
        as_array(v, "target types")?
            .iter()
            .map(|t| {
                let c = as_uint(t, "valtype")?;
                ValType::from_code(c).ok_or(DecodeError::UnknownValType(c))
            })
            .collect()
    };
    Ok(Target {
        name,
        params: types(&items[1])?,
        results: types(&items[2])?,
    })
}

fn decode_refines(v: &Value) -> Result<Refines, DecodeError> {
    let items = as_array(v, "refines")?;
    if items.len() < 3 || items.len() > 4 {
        return Err(DecodeError::BadShape("refines arity"));
    }
    let hash: [u8; 32] = match &items[0] {
        Value::Bytes(b) => b
            .as_slice()
            .try_into()
            .map_err(|_| DecodeError::BadShape("refines hash length"))?,
        _ => return Err(DecodeError::BadShape("refines hash")),
    };
    let func = match &items[1] {
        Value::Text(s) => s.clone(),
        _ => return Err(DecodeError::BadShape("refines func")),
    };
    let over = as_array(&items[2], "refines over")?
        .iter()
        .map(decode_expr)
        .collect::<Result<Vec<_>, _>>()?;
    let assuming = match items.get(3) {
        Some(a) => Some(decode_expr(a)?),
        None => None,
    };
    Ok(Refines {
        artifact_sha256: hash,
        func,
        over,
        assuming,
    })
}

/// Strict decode + structural validation. The input must be canonical: we
/// re-encode and require byte equality, so a non-canonical (but parseable)
/// encoding is rejected rather than silently accepted under a different hash.
pub fn decode_contract(bytes: &[u8]) -> Result<Contract, DecodeError> {
    let v = cbor::from_bytes(bytes)?;
    let entries = match &v {
        Value::Map(e) => e,
        _ => return Err(DecodeError::BadShape("top-level not a map")),
    };
    let mut version = None;
    let mut target = None;
    let mut assume = Vec::new();
    let mut pre = None;
    let mut post = None;
    let mut frame = None;
    let mut refines = None;
    let mut features = 0u64;
    for (k, val) in entries {
        match *k {
            K_VERSION => version = Some(as_uint(val, "version")?),
            K_TARGET => target = Some(decode_target(val)?),
            K_ASSUME => {
                for f in as_array(val, "assume")? {
                    let c = as_uint(f, "assume flag")?;
                    assume.push(AssumeFlag::from_code(c).ok_or(DecodeError::UnknownAssumeFlag(c))?);
                }
            }
            K_PRE => pre = Some(decode_expr(val)?),
            K_POST => post = Some(decode_expr(val)?),
            K_FRAME => {
                frame = Some(
                    as_array(val, "frame")?
                        .iter()
                        .map(decode_expr)
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            K_REFINES => refines = Some(decode_refines(val)?),
            K_FEATURES => features = as_uint(val, "features")?,
            other => return Err(DecodeError::UnknownTopLevelKey(other)),
        }
    }
    let version = version.ok_or(DecodeError::MissingField(K_VERSION))?;
    if version != 0 {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    if features != 0 {
        return Err(DecodeError::UnknownFeatureBits(features));
    }
    let c = Contract {
        version: 0,
        target: target.ok_or(DecodeError::MissingField(K_TARGET))?,
        assume,
        pre: pre.ok_or(DecodeError::MissingField(K_PRE))?,
        post: post.ok_or(DecodeError::MissingField(K_POST))?,
        frame: frame.ok_or(DecodeError::MissingField(K_FRAME))?,
        refines,
        features: 0,
    };
    // Canonicality check: bytes must equal the re-encoding of the parse.
    if encode_contract(&c) != bytes {
        return Err(DecodeError::Cbor(CborError::NonShortestForm));
    }
    c.validate().map_err(DecodeError::Validate)?;
    Ok(c)
}
