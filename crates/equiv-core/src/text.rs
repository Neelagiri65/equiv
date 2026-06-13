//! The S-expression text surface. NOT frozen; sugar that compiles 1:1 to the
//! canonical binary form. Grammar follows the worked examples in
//! docs/spec-eqc-0-draft.md §8.

use crate::ast::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    Lex(String),
    Parse(String),
}

fn perr<T>(msg: impl Into<String>) -> Result<T, TextError> {
    Err(TextError::Parse(msg.into()))
}

// ---------- s-expressions ----------

#[derive(Debug, Clone, PartialEq, Eq)]
enum SExp {
    Atom(String),
    Str(String),
    List(Vec<SExp>),
}

fn lex(src: &str) -> Result<Vec<SExp>, TextError> {
    let mut chars = src.chars().peekable();
    let mut stack: Vec<Vec<SExp>> = vec![Vec::new()];
    while let Some(&c) = chars.peek() {
        match c {
            ';' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                stack.push(Vec::new());
            }
            ')' => {
                chars.next();
                let done = stack.pop().ok_or_else(|| TextError::Lex("unbalanced )".into()))?;
                let top = stack
                    .last_mut()
                    .ok_or_else(|| TextError::Lex("unbalanced )".into()))?;
                top.push(SExp::List(done));
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some(c) => s.push(c),
                        None => return Err(TextError::Lex("unterminated string".into())),
                    }
                }
                stack.last_mut().unwrap().push(SExp::Str(s));
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' || c == ';' || c == '"' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                stack.last_mut().unwrap().push(SExp::Atom(s));
            }
        }
    }
    if stack.len() != 1 {
        return Err(TextError::Lex("unbalanced (".into()));
    }
    Ok(stack.pop().unwrap())
}

fn parse_uint(atom: &str) -> Result<u64, TextError> {
    let (digits, radix) = if let Some(h) = atom.strip_prefix("0x").or_else(|| atom.strip_prefix("#x")) {
        (h, 16)
    } else {
        (atom, 10)
    };
    u64::from_str_radix(digits, radix).map_err(|_| TextError::Parse(format!("bad number `{atom}`")))
}

// ---------- expression parsing ----------

struct Ctx<'a> {
    params: &'a [String],
}

impl Ctx<'_> {
    fn arg(&self, name: &str) -> Result<Expr, TextError> {
        match self.params.iter().position(|p| p == name) {
            Some(i) => Ok(Expr::Arg(i as u8)),
            None => perr(format!("unknown parameter `{name}`")),
        }
    }
}

fn bv_op(name: &str) -> Option<BvOp> {
    Some(match name {
        "add" => BvOp::Add,
        "sub" => BvOp::Sub,
        "mul" => BvOp::Mul,
        "div_u" => BvOp::DivU,
        "div_s" => BvOp::DivS,
        "rem_u" => BvOp::RemU,
        "rem_s" => BvOp::RemS,
        "bvand" => BvOp::And,
        "bvor" => BvOp::Or,
        "bvxor" => BvOp::Xor,
        "shl" => BvOp::Shl,
        "shr_u" => BvOp::ShrU,
        "shr_s" => BvOp::ShrS,
        _ => return None,
    })
}

fn cmp_op(name: &str) -> Option<CmpOp> {
    Some(match name {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "lt_u" => CmpOp::LtU,
        "lt_s" => CmpOp::LtS,
        "le_u" => CmpOp::LeU,
        "le_s" => CmpOp::LeS,
        "gt_u" => CmpOp::GtU,
        "gt_s" => CmpOp::GtS,
        "ge_u" => CmpOp::GeU,
        "ge_s" => CmpOp::GeS,
        _ => return None,
    })
}

fn parse_state(s: &SExp) -> Result<State, TextError> {
    match s {
        SExp::Atom(a) if a == "old" => Ok(State::Old),
        SExp::Atom(a) if a == "new" => Ok(State::New),
        _ => perr("expected `old` or `new`"),
    }
}

fn fold_nary(
    items: &[SExp],
    ctx: &Ctx,
    join: fn(Box<Expr>, Box<Expr>) -> Expr,
    what: &str,
) -> Result<Expr, TextError> {
    if items.len() < 2 {
        return perr(format!("`{what}` needs at least 2 arguments"));
    }
    let mut exprs = items.iter().map(|s| parse_expr(s, ctx));
    let mut acc = exprs.next().unwrap()?;
    for e in exprs {
        acc = join(Box::new(acc), Box::new(e?));
    }
    Ok(acc)
}

fn parse_expr(s: &SExp, ctx: &Ctx) -> Result<Expr, TextError> {
    match s {
        SExp::Atom(a) if a == "result" => Ok(Expr::Result),
        SExp::Atom(a) if a == "idx" => Ok(Expr::Idx),
        SExp::Atom(a) if a.starts_with('$') => ctx.arg(a),
        SExp::Atom(a) => perr(format!("unexpected atom `{a}` (literals must be (c32 N) or (c64 N))")),
        SExp::Str(_) => perr("unexpected string in expression"),
        SExp::List(items) => {
            let head = match items.first() {
                Some(SExp::Atom(a)) => a.as_str(),
                _ => return perr("expected operator"),
            };
            let args = &items[1..];
            let one = |i: usize| parse_expr(&args[i], ctx);
            let need = |n: usize| -> Result<(), TextError> {
                if args.len() == n {
                    Ok(())
                } else {
                    perr(format!("`{head}` expects {n} argument(s), got {}", args.len()))
                }
            };
            if let Some(o) = bv_op(head) {
                need(2)?;
                return Ok(Expr::Bv(o, one(0)?.into(), one(1)?.into()));
            }
            if let Some(o) = cmp_op(head) {
                need(2)?;
                return Ok(Expr::Cmp(o, one(0)?.into(), one(1)?.into()));
            }
            match head {
                "c32" => {
                    need(1)?;
                    match &args[0] {
                        SExp::Atom(a) => {
                            let n = parse_uint(a)?;
                            let n = u32::try_from(n)
                                .map_err(|_| TextError::Parse(format!("c32 out of range: {n}")))?;
                            Ok(Expr::C32(n))
                        }
                        _ => perr("c32 expects a number"),
                    }
                }
                "c64" => {
                    need(1)?;
                    match &args[0] {
                        SExp::Atom(a) => Ok(Expr::C64(parse_uint(a)?)),
                        _ => perr("c64 expects a number"),
                    }
                }
                "and" => fold_nary(args, ctx, Expr::Band, "and"),
                "or" => fold_nary(args, ctx, Expr::Bor, "or"),
                "not" => {
                    need(1)?;
                    Ok(Expr::Bnot(one(0)?.into()))
                }
                "implies" => {
                    need(2)?;
                    Ok(Expr::Implies(one(0)?.into(), one(1)?.into()))
                }
                "ite" => {
                    need(3)?;
                    Ok(Expr::Ite(one(0)?.into(), one(1)?.into(), one(2)?.into()))
                }
                "zext" => {
                    need(1)?;
                    Ok(Expr::Zext(one(0)?.into()))
                }
                "wrap" => {
                    need(1)?;
                    Ok(Expr::Wrap(one(0)?.into()))
                }
                "byte" | "word32" | "word64" => {
                    need(2)?;
                    let st = parse_state(&args[0])?;
                    let addr = parse_expr(&args[1], ctx)?;
                    Ok(match head {
                        "byte" => Expr::Byte(st, addr.into()),
                        "word32" => Expr::Word32(st, addr.into()),
                        _ => Expr::Word64(st, addr.into()),
                    })
                }
                "range" => {
                    need(2)?;
                    Ok(Expr::Range(one(0)?.into(), one(1)?.into()))
                }
                "readable" => {
                    need(1)?;
                    Ok(Expr::Readable(one(0)?.into()))
                }
                "writable" => {
                    need(1)?;
                    Ok(Expr::Writable(one(0)?.into()))
                }
                "disjoint" => {
                    need(2)?;
                    Ok(Expr::Disjoint(one(0)?.into(), one(1)?.into()))
                }
                "mem-eq" => {
                    need(4)?;
                    Ok(Expr::MemEq(
                        parse_state(&args[0])?,
                        parse_expr(&args[1], ctx)?.into(),
                        parse_state(&args[2])?,
                        parse_expr(&args[3], ctx)?.into(),
                    ))
                }
                "unchanged" => {
                    need(1)?;
                    Ok(Expr::Unchanged(one(0)?.into()))
                }
                "forall_b" => {
                    need(3)?;
                    let k = match &args[0] {
                        SExp::Atom(a) => {
                            let n = parse_uint(a)?;
                            u32::try_from(n)
                                .map_err(|_| TextError::Parse(format!("forall_b bound too large: {n}")))?
                        }
                        _ => return perr("forall_b expects a literal bound"),
                    };
                    Ok(Expr::ForallB(k, one(1)?.into(), one(2)?.into()))
                }
                other => perr(format!("unknown operator `{other}`")),
            }
        }
    }
}

// ---------- contract parsing ----------

fn parse_valtype(a: &str) -> Result<ValType, TextError> {
    Ok(match a {
        "i32" => ValType::I32,
        "i64" => ValType::I64,
        "f32" => ValType::F32,
        "f64" => ValType::F64,
        "v128" => ValType::V128,
        other => return perr(format!("unknown value type `{other}`")),
    })
}

fn parse_target(s: &SExp) -> Result<(Target, Vec<String>), TextError> {
    // (target (func "name" (param $x i32)* (result i32)*))
    let items = match s {
        SExp::List(items) => items,
        _ => return perr("bad target"),
    };
    let func = match items.as_slice() {
        [SExp::Atom(t), f] if t == "target" => f,
        _ => return perr("expected (target (func ...))"),
    };
    let fitems = match func {
        SExp::List(items) => items,
        _ => return perr("bad func"),
    };
    match fitems.first() {
        Some(SExp::Atom(a)) if a == "func" => {}
        _ => return perr("expected (func ...)"),
    }
    let name = match fitems.get(1) {
        Some(SExp::Str(s)) => s.clone(),
        _ => return perr("func needs a quoted name"),
    };
    let mut params = Vec::new();
    let mut param_names = Vec::new();
    let mut results = Vec::new();
    for clause in &fitems[2..] {
        let citems = match clause {
            SExp::List(items) => items,
            _ => return perr("bad func clause"),
        };
        match citems.as_slice() {
            [SExp::Atom(k), SExp::Atom(n), SExp::Atom(t)] if k == "param" && n.starts_with('$') => {
                param_names.push(n.clone());
                params.push(parse_valtype(t)?);
            }
            [SExp::Atom(k), SExp::Atom(t)] if k == "result" => {
                results.push(parse_valtype(t)?);
            }
            _ => return perr("expected (param $name type) or (result type)"),
        }
    }
    Ok((Target { name, params, results }, param_names))
}

fn parse_assume(items: &[SExp]) -> Result<Vec<AssumeFlag>, TextError> {
    let mut flags = Vec::new();
    for it in items {
        match it {
            SExp::Atom(a) => flags.push(match a.as_str() {
                "no-imports" => AssumeFlag::NoImports,
                "no-indirect" => AssumeFlag::NoIndirect,
                "no-memory-grow" => AssumeFlag::NoMemoryGrow,
                "no-trap" => AssumeFlag::NoTrap,
                other => return perr(format!("unknown assume flag `{other}`")),
            }),
            SExp::List(inner) => match inner.split_first() {
                Some((SExp::Atom(k), rest)) if k == "intrinsics" => {
                    for r in rest {
                        match r {
                            SExp::Atom(a) => flags.push(match a.as_str() {
                                "alloc" => AssumeFlag::IntrinsicsAlloc,
                                "bulk-memory" => AssumeFlag::IntrinsicsBulkMemory,
                                "panic-abort" => AssumeFlag::IntrinsicsPanicAbort,
                                other => return perr(format!("unknown intrinsic `{other}`")),
                            }),
                            _ => return perr("bad intrinsics clause"),
                        }
                    }
                }
                _ => return perr("bad assume clause"),
            },
            _ => return perr("bad assume clause"),
        }
    }
    flags.sort_unstable();
    flags.dedup();
    Ok(flags)
}

fn parse_hash(a: &str) -> Result<[u8; 32], TextError> {
    let hex = a
        .strip_prefix("sha256:")
        .ok_or_else(|| TextError::Parse("artifact hash must be sha256:<hex>".into()))?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return perr("artifact hash must be 64 hex chars");
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    Ok(out)
}

fn parse_refines(args: &[SExp], ctx: &Ctx) -> Result<Refines, TextError> {
    // (refines (artifact sha256:HEX) "func" (over expr*) (assuming expr)?)
    let hash = match args.first() {
        Some(SExp::List(items)) => match items.as_slice() {
            [SExp::Atom(k), SExp::Atom(h)] if k == "artifact" => parse_hash(h)?,
            _ => return perr("expected (artifact sha256:<hex>)"),
        },
        _ => return perr("refines needs (artifact ...)"),
    };
    let func = match args.get(1) {
        Some(SExp::Str(s)) => s.clone(),
        _ => return perr("refines needs a quoted function name"),
    };
    let mut over = Vec::new();
    let mut assuming = None;
    for clause in &args[2..] {
        let citems = match clause {
            SExp::List(items) => items,
            _ => return perr("bad refines clause"),
        };
        match citems.split_first() {
            Some((SExp::Atom(k), rest)) if k == "over" => {
                for r in rest {
                    over.push(parse_expr(r, ctx)?);
                }
            }
            Some((SExp::Atom(k), rest)) if k == "assuming" && rest.len() == 1 => {
                assuming = Some(parse_expr(&rest[0], ctx)?);
            }
            _ => return perr("expected (over ...) or (assuming ...)"),
        }
    }
    Ok(Refines {
        artifact_sha256: hash,
        func,
        over,
        assuming,
    })
}

/// Parse the text surface into a validated contract.
pub fn parse_contract(src: &str) -> Result<Contract, TextError> {
    let top = lex(src)?;
    let items = match top.as_slice() {
        [SExp::List(items)] => items,
        _ => return perr("expected a single (contract ...) form"),
    };
    match items.first() {
        Some(SExp::Atom(a)) if a == "contract" => {}
        _ => return perr("expected (contract ...)"),
    }
    match items.get(1) {
        Some(SExp::Atom(v)) if v == "eqc/0" => {}
        _ => return perr("expected version atom `eqc/0`"),
    }
    let mut target = None;
    let mut param_names = Vec::new();
    let mut assume = Vec::new();
    let mut pre = None;
    let mut post = None;
    let mut frame = None;
    let mut refines_clause = None;
    for clause in &items[2..] {
        let citems = match clause {
            SExp::List(items) => items,
            _ => return perr("bad top-level clause"),
        };
        let head = match citems.first() {
            Some(SExp::Atom(a)) => a.clone(),
            _ => return perr("bad top-level clause"),
        };
        match head.as_str() {
            "target" => {
                let (t, names) = parse_target(clause)?;
                target = Some(t);
                param_names = names;
            }
            "assume" => assume = parse_assume(&citems[1..])?,
            "pre" | "post" => {
                if citems.len() != 2 {
                    return perr(format!("`{head}` expects exactly one expression"));
                }
                let ctx = Ctx { params: &param_names };
                let e = parse_expr(&citems[1], &ctx)?;
                if head == "pre" {
                    pre = Some(e);
                } else {
                    post = Some(e);
                }
            }
            "frame" => {
                let ctx = Ctx { params: &param_names };
                frame = Some(
                    citems[1..]
                        .iter()
                        .map(|s| parse_expr(s, &ctx))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            "refines" => {
                let ctx = Ctx { params: &param_names };
                refines_clause = Some(parse_refines(&citems[1..], &ctx)?);
            }
            other => return perr(format!("unknown clause `{other}`")),
        }
    }
    let c = Contract {
        version: 0,
        target: target.ok_or_else(|| TextError::Parse("missing (target ...)".into()))?,
        assume,
        pre: pre.ok_or_else(|| TextError::Parse("missing (pre ...)".into()))?,
        post: post.ok_or_else(|| TextError::Parse("missing (post ...)".into()))?,
        frame: frame.ok_or_else(|| TextError::Parse("missing (frame ...)".into()))?,
        refines: refines_clause,
        features: 0,
    };
    c.validate()
        .map_err(|e| TextError::Parse(format!("validation: {e:?}")))?;
    Ok(c)
}
