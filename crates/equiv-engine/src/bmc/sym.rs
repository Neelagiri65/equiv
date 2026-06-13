//! Symbolic execution of the supported WASM subset into the term DAG.
//! v0 envelope: straight-line code, balanced if/else, select. No loops,
//! no br/return, no memory, no calls, no div/rem, i32/i64 only. Anything
//! outside extracts as `Unsupported` and the artifact routes to difftest.

use super::term::{Arena, TId, Term};
use equiv_core::ast::{BvOp, CmpOp, ValType};
use wasmparser::{Operator, Parser, Payload};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiniOp {
    I32Const(i32),
    I64Const(i64),
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    Bin(u8, BvOp), // width, op
    Cmp(u8, CmpOp),
    Eqz(u8),
    ExtendU,
    ExtendS,
    Wrap,
    Select,
    Drop,
    Nop,
    Block,
    Loop,
    If,
    Else,
    End,
    Br(u32),
    BrIf(u32),
    Return,
    Unreachable,
}

pub struct FuncBody {
    pub params: Vec<ValType>,
    pub n_results: usize,
    pub locals: Vec<ValType>, // declared locals (excluding params)
    pub ops: Vec<MiniOp>,
}

fn vt(t: wasmparser::ValType) -> Option<ValType> {
    match t {
        wasmparser::ValType::I32 => Some(ValType::I32),
        wasmparser::ValType::I64 => Some(ValType::I64),
        _ => None,
    }
}

fn mini(op: &Operator) -> Option<MiniOp> {
    use BvOp::*;
    use CmpOp::*;
    Some(match op {
        Operator::I32Const { value } => MiniOp::I32Const(*value),
        Operator::I64Const { value } => MiniOp::I64Const(*value),
        Operator::LocalGet { local_index } => MiniOp::LocalGet(*local_index),
        Operator::LocalSet { local_index } => MiniOp::LocalSet(*local_index),
        Operator::LocalTee { local_index } => MiniOp::LocalTee(*local_index),
        Operator::I32Add => MiniOp::Bin(32, Add),
        Operator::I32Sub => MiniOp::Bin(32, Sub),
        Operator::I32Mul => MiniOp::Bin(32, Mul),
        Operator::I32And => MiniOp::Bin(32, And),
        Operator::I32Or => MiniOp::Bin(32, Or),
        Operator::I32Xor => MiniOp::Bin(32, Xor),
        Operator::I32Shl => MiniOp::Bin(32, Shl),
        Operator::I32ShrU => MiniOp::Bin(32, ShrU),
        Operator::I32ShrS => MiniOp::Bin(32, ShrS),
        Operator::I64Add => MiniOp::Bin(64, Add),
        Operator::I64Sub => MiniOp::Bin(64, Sub),
        Operator::I64Mul => MiniOp::Bin(64, Mul),
        Operator::I64And => MiniOp::Bin(64, And),
        Operator::I64Or => MiniOp::Bin(64, Or),
        Operator::I64Xor => MiniOp::Bin(64, Xor),
        Operator::I64Shl => MiniOp::Bin(64, Shl),
        Operator::I64ShrU => MiniOp::Bin(64, ShrU),
        Operator::I64ShrS => MiniOp::Bin(64, ShrS),
        Operator::I32Eq => MiniOp::Cmp(32, Eq),
        Operator::I32Ne => MiniOp::Cmp(32, Ne),
        Operator::I32LtU => MiniOp::Cmp(32, LtU),
        Operator::I32LtS => MiniOp::Cmp(32, LtS),
        Operator::I32LeU => MiniOp::Cmp(32, LeU),
        Operator::I32LeS => MiniOp::Cmp(32, LeS),
        Operator::I32GtU => MiniOp::Cmp(32, GtU),
        Operator::I32GtS => MiniOp::Cmp(32, GtS),
        Operator::I32GeU => MiniOp::Cmp(32, GeU),
        Operator::I32GeS => MiniOp::Cmp(32, GeS),
        Operator::I64Eq => MiniOp::Cmp(64, Eq),
        Operator::I64Ne => MiniOp::Cmp(64, Ne),
        Operator::I64LtU => MiniOp::Cmp(64, LtU),
        Operator::I64LtS => MiniOp::Cmp(64, LtS),
        Operator::I64LeU => MiniOp::Cmp(64, LeU),
        Operator::I64LeS => MiniOp::Cmp(64, LeS),
        Operator::I64GtU => MiniOp::Cmp(64, GtU),
        Operator::I64GtS => MiniOp::Cmp(64, GtS),
        Operator::I64GeU => MiniOp::Cmp(64, GeU),
        Operator::I64GeS => MiniOp::Cmp(64, GeS),
        Operator::I32Eqz => MiniOp::Eqz(32),
        Operator::I64Eqz => MiniOp::Eqz(64),
        Operator::I64ExtendI32U => MiniOp::ExtendU,
        Operator::I64ExtendI32S => MiniOp::ExtendS,
        Operator::I32WrapI64 => MiniOp::Wrap,
        Operator::Select => MiniOp::Select,
        Operator::Drop => MiniOp::Drop,
        Operator::Nop => MiniOp::Nop,
        Operator::Block { blockty } | Operator::If { blockty } | Operator::Loop { blockty } => {
            // Only empty or single-result block types in v0.
            match blockty {
                wasmparser::BlockType::Empty | wasmparser::BlockType::Type(_) => {}
                wasmparser::BlockType::FuncType(_) => return None,
            }
            match op {
                Operator::Block { .. } => MiniOp::Block,
                Operator::Loop { .. } => MiniOp::Loop,
                _ => MiniOp::If,
            }
        }
        Operator::Else => MiniOp::Else,
        Operator::End => MiniOp::End,
        Operator::Br { relative_depth } => MiniOp::Br(*relative_depth),
        Operator::BrIf { relative_depth } => MiniOp::BrIf(*relative_depth),
        Operator::Return => MiniOp::Return,
        Operator::Unreachable => MiniOp::Unreachable,
        _ => return None,
    })
}

/// Extract the exported target function. Returns None if not found or if it
/// uses anything outside the v0 BMC envelope.
pub fn extract(bytes: &[u8], export_name: &str) -> Option<FuncBody> {
    let mut func_types: Vec<(Vec<ValType>, usize)> = Vec::new(); // (params, n_results)
    let mut func_type_idx: Vec<u32> = Vec::new();
    let mut n_imported = 0u32;
    let mut target_func: Option<u32> = None;
    let mut bodies: Vec<wasmparser::FunctionBody> = Vec::new();

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.ok()? {
            Payload::TypeSection(reader) => {
                for group in reader {
                    for st in group.ok()?.types() {
                        let ft = match &st.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(ft) => ft,
                            _ => return None,
                        };
                        let params: Option<Vec<ValType>> = ft.params().iter().map(|t| vt(*t)).collect();
                        func_types.push((params?, ft.results().len()));
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for imp in reader {
                    if matches!(imp.ok()?.ty, wasmparser::TypeRef::Func(_)) {
                        n_imported += 1;
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ti in reader {
                    func_type_idx.push(ti.ok()?);
                }
            }
            Payload::ExportSection(reader) => {
                for ex in reader {
                    let ex = ex.ok()?;
                    if ex.kind == wasmparser::ExternalKind::Func && ex.name == export_name {
                        target_func = Some(ex.index);
                    }
                }
            }
            Payload::CodeSectionEntry(body) => bodies.push(body),
            _ => {}
        }
    }

    let fidx = target_func?;
    let code_idx = fidx.checked_sub(n_imported)? as usize;
    let body = bodies.get(code_idx)?;
    let ty = &func_types[*func_type_idx.get(code_idx)? as usize];

    let mut locals = Vec::new();
    for l in body.get_locals_reader().ok()? {
        let (count, t) = l.ok()?;
        let t = vt(t)?;
        for _ in 0..count {
            locals.push(t);
        }
    }
    let mut ops = Vec::new();
    let mut reader = body.get_operators_reader().ok()?;
    while !reader.eof() {
        ops.push(mini(&reader.read().ok()?)?);
    }
    Some(FuncBody {
        params: ty.0.clone(),
        n_results: ty.1,
        locals,
        ops,
    })
}

// ---------- symbolic execution ----------
//
// Path-condition executor: every control join ite-merges states under
// absolute (whole-function) path conditions. Loops unroll up to `unroll_k`;
// a live back-edge after the last unrolling becomes an `incomplete`
// condition. `proved` then requires (pre ∧ incomplete) UNSAT, i.e. the
// unwinding assertion (graph-refine's Restrict rule / CBMC
// --unwinding-assertions).

#[derive(Clone)]
struct State {
    locals: Vec<TId>,
    stack: Vec<TId>,
}

/// One arrival at a label: (absolute path condition, state).
type Arrival = (TId, State);

#[derive(Default)]
struct RegionOut {
    /// Fell off the region's end.
    fall: Option<Arrival>,
    /// Branches to labels: (relative depth seen from THIS region, cond, state).
    exits: Vec<(u32, TId, State)>,
    /// Returns from the function: (cond, result stack values).
    rets: Vec<(TId, Vec<TId>)>,
    /// Conditions under which a trap (unreachable) executes.
    traps: Vec<TId>,
    /// Conditions under which a loop exceeded the unroll bound.
    incompletes: Vec<TId>,
    /// Position just after this region's terminating End/Else.
    after: usize,
    was_else: bool,
}

/// Skip unreachable code to the region's End/Else at depth 0.
fn skip_region(ops: &[MiniOp], mut i: usize) -> Result<(usize, bool), ()> {
    let mut depth = 0u32;
    while i < ops.len() {
        match ops[i] {
            MiniOp::Block | MiniOp::Loop | MiniOp::If => depth += 1,
            MiniOp::Else if depth == 0 => return Ok((i + 1, true)),
            MiniOp::End => {
                if depth == 0 {
                    return Ok((i + 1, false));
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    Err(())
}

/// Merge arrivals into one (cond, state); None if no arrivals (unreachable).
fn merge(arrivals: Vec<Arrival>, ar: &mut Arena) -> Result<Option<Arrival>, ()> {
    let mut it = arrivals.into_iter();
    let Some((mut cond, mut st)) = it.next() else {
        return Ok(None);
    };
    for (c, s) in it {
        if s.locals.len() != st.locals.len() || s.stack.len() != st.stack.len() {
            return Err(());
        }
        let pick = |ar: &mut Arena, a: TId, b: TId| {
            if a == b {
                a
            } else {
                let w = ar.width(a);
                ar.add(Term::Ite { c, a, b, w })
            }
        };
        st.locals = s
            .locals
            .iter()
            .zip(&st.locals)
            .map(|(a, b)| pick(ar, *a, *b))
            .collect();
        st.stack = s
            .stack
            .iter()
            .zip(&st.stack)
            .map(|(a, b)| pick(ar, *a, *b))
            .collect();
        cond = ar.bor(cond, c);
    }
    Ok(Some((cond, st)))
}

struct Sim<'a> {
    ops: &'a [MiniOp],
    unroll_k: u32,
    n_results: usize,
}

impl Sim<'_> {
    /// Simulate a region starting at `i` under absolute condition `cond`.
    fn region(&self, mut i: usize, mut st: State, mut cond: TId, ar: &mut Arena) -> Result<RegionOut, ()> {
        let mut out = RegionOut::default();
        macro_rules! dead_end {
            () => {{
                let (after, was_else) = skip_region(self.ops, i)?;
                out.after = after;
                out.was_else = was_else;
                return Ok(out);
            }};
        }
        while i < self.ops.len() {
            let op = self.ops[i];
            i += 1;
            match op {
                MiniOp::End => {
                    out.fall = Some((cond, st));
                    out.after = i;
                    return Ok(out);
                }
                MiniOp::Else => {
                    out.fall = Some((cond, st));
                    out.after = i;
                    out.was_else = true;
                    return Ok(out);
                }
                MiniOp::Nop => {}
                MiniOp::Drop => {
                    st.stack.pop().ok_or(())?;
                }
                MiniOp::I32Const(v) => st.stack.push(ar.c(32, v as u32 as u64)),
                MiniOp::I64Const(v) => st.stack.push(ar.c(64, v as u64)),
                MiniOp::LocalGet(n) => st.stack.push(*st.locals.get(n as usize).ok_or(())?),
                MiniOp::LocalSet(n) => {
                    let v = st.stack.pop().ok_or(())?;
                    *st.locals.get_mut(n as usize).ok_or(())? = v;
                }
                MiniOp::LocalTee(n) => {
                    let v = *st.stack.last().ok_or(())?;
                    *st.locals.get_mut(n as usize).ok_or(())? = v;
                }
                MiniOp::Bin(w, o) => {
                    let b = st.stack.pop().ok_or(())?;
                    let a = st.stack.pop().ok_or(())?;
                    st.stack.push(ar.add(Term::Bin { op: o, w, a, b }));
                }
                MiniOp::Cmp(_, o) => {
                    let b = st.stack.pop().ok_or(())?;
                    let a = st.stack.pop().ok_or(())?;
                    let bit = ar.add(Term::Cmp { op: o, a, b });
                    st.stack.push(ar.add(Term::Ext { a: bit, w: 32, signed: false }));
                }
                MiniOp::Eqz(_) => {
                    let a = st.stack.pop().ok_or(())?;
                    let nz = ar.add(Term::NeZero { a });
                    let z = ar.bnot(nz);
                    st.stack.push(ar.add(Term::Ext { a: z, w: 32, signed: false }));
                }
                MiniOp::ExtendU => {
                    let a = st.stack.pop().ok_or(())?;
                    st.stack.push(ar.add(Term::Ext { a, w: 64, signed: false }));
                }
                MiniOp::ExtendS => {
                    let a = st.stack.pop().ok_or(())?;
                    st.stack.push(ar.add(Term::Ext { a, w: 64, signed: true }));
                }
                MiniOp::Wrap => {
                    let a = st.stack.pop().ok_or(())?;
                    st.stack.push(ar.add(Term::Slice { a, w: 32 }));
                }
                MiniOp::Select => {
                    let c = st.stack.pop().ok_or(())?;
                    let b = st.stack.pop().ok_or(())?;
                    let a = st.stack.pop().ok_or(())?;
                    let cb = ar.add(Term::NeZero { a: c });
                    let w = ar.width(a);
                    st.stack.push(ar.add(Term::Ite { c: cb, a, b, w }));
                }
                MiniOp::Unreachable => {
                    out.traps.push(cond);
                    dead_end!();
                }
                MiniOp::Return => {
                    let mut vals = Vec::new();
                    for _ in 0..self.n_results {
                        vals.push(st.stack.pop().ok_or(())?);
                    }
                    vals.reverse();
                    out.rets.push((cond, vals));
                    dead_end!();
                }
                MiniOp::Br(d) => {
                    out.exits.push((d, cond, st.clone()));
                    dead_end!();
                }
                MiniOp::BrIf(d) => {
                    let t = st.stack.pop().ok_or(())?;
                    let tb = ar.add(Term::NeZero { a: t });
                    let taken = ar.band(cond, tb);
                    out.exits.push((d, taken, st.clone()));
                    let ntb = ar.bnot(tb);
                    cond = ar.band(cond, ntb);
                }
                MiniOp::Block => {
                    let sub = self.region(i, st.clone(), cond, ar)?;
                    if sub.was_else {
                        return Err(());
                    }
                    i = sub.after;
                    let mut arrivals: Vec<Arrival> = Vec::new();
                    if let Some(f) = sub.fall {
                        arrivals.push(f);
                    }
                    self.absorb(&mut out, sub.exits, sub.rets, sub.traps, sub.incompletes, &mut arrivals);
                    match merge(arrivals, ar)? {
                        Some((c2, s2)) => {
                            cond = c2;
                            st = s2;
                        }
                        None => dead_end!(),
                    }
                }
                MiniOp::If => {
                    let t = st.stack.pop().ok_or(())?;
                    let tb = ar.add(Term::NeZero { a: t });
                    let then_cond = ar.band(cond, tb);
                    let ntb = ar.bnot(tb);
                    let else_cond = ar.band(cond, ntb);
                    let base = st.clone();

                    let then_out = self.region(i, st.clone(), then_cond, ar)?;
                    let mut arrivals: Vec<Arrival> = Vec::new();
                    if let Some(f) = then_out.fall {
                        arrivals.push(f);
                    }
                    self.absorb(
                        &mut out,
                        then_out.exits,
                        then_out.rets,
                        then_out.traps,
                        then_out.incompletes,
                        &mut arrivals,
                    );
                    let after = if then_out.was_else {
                        let else_out = self.region(then_out.after, base, else_cond, ar)?;
                        if else_out.was_else {
                            return Err(());
                        }
                        if let Some(f) = else_out.fall {
                            arrivals.push(f);
                        }
                        self.absorb(
                            &mut out,
                            else_out.exits,
                            else_out.rets,
                            else_out.traps,
                            else_out.incompletes,
                            &mut arrivals,
                        );
                        else_out.after
                    } else {
                        arrivals.push((else_cond, base));
                        then_out.after
                    };
                    i = after;
                    match merge(arrivals, ar)? {
                        Some((c2, s2)) => {
                            cond = c2;
                            st = s2;
                        }
                        None => dead_end!(),
                    }
                }
                MiniOp::Loop => {
                    let body_start = i;
                    let mut entry: Option<Arrival> = Some((cond, st.clone()));
                    let mut loop_arrivals: Vec<Arrival> = Vec::new();
                    let mut after = None;
                    for iter in 0..self.unroll_k + 1 {
                        let Some((ec, es)) = entry.take() else { break };
                        if iter == self.unroll_k {
                            // Bound exhausted with a live back edge.
                            out.incompletes.push(ec);
                            break;
                        }
                        let sub = self.region(body_start, es, ec, ar)?;
                        if sub.was_else {
                            return Err(());
                        }
                        after = Some(sub.after);
                        if let Some(f) = sub.fall {
                            loop_arrivals.push(f); // fell off loop bottom: exits loop
                        }
                        let mut backs: Vec<Arrival> = Vec::new();
                        for (d, c, s) in sub.exits {
                            if d == 0 {
                                backs.push((c, s)); // br to loop header
                            } else {
                                out.exits.push((d - 1, c, s));
                            }
                        }
                        out.rets.extend(sub.rets);
                        out.traps.extend(sub.traps);
                        out.incompletes.extend(sub.incompletes);
                        entry = merge(backs, ar)?;
                    }
                    let Some(after) = after else {
                        // Body never simulated (k = 0): treat as incomplete.
                        let (a, was_else) = skip_region(self.ops, body_start)?;
                        if was_else {
                            return Err(());
                        }
                        out.after = a;
                        return Ok(out);
                    };
                    i = after;
                    match merge(loop_arrivals, ar)? {
                        Some((c2, s2)) => {
                            cond = c2;
                            st = s2;
                        }
                        None => dead_end!(),
                    }
                }
            }
        }
        Err(()) // ran out of ops without a terminating End
    }

    /// Route a subregion's pending control flow: depth-0 exits become
    /// arrivals at the label we just closed; deeper exits propagate up.
    fn absorb(
        &self,
        out: &mut RegionOut,
        exits: Vec<(u32, TId, State)>,
        rets: Vec<(TId, Vec<TId>)>,
        traps: Vec<TId>,
        incompletes: Vec<TId>,
        arrivals: &mut Vec<Arrival>,
    ) {
        for (d, c, s) in exits {
            if d == 0 {
                arrivals.push((c, s));
            } else {
                out.exits.push((d - 1, c, s));
            }
        }
        out.rets.extend(rets);
        out.traps.extend(traps);
        out.incompletes.extend(incompletes);
    }
}

pub struct SymResult {
    pub result: Option<TId>,
    /// Condition under which the function traps (1-bit), if any path can.
    pub trap: Option<TId>,
    /// Condition under which a loop bound was exceeded, if any.
    pub incomplete: Option<TId>,
    pub unroll_k: u32,
}

fn w_of(t: ValType) -> u8 {
    match t {
        ValType::I64 => 64,
        _ => 32,
    }
}

fn or_all(conds: &[TId], ar: &mut Arena) -> Option<TId> {
    let mut it = conds.iter().copied();
    let first = it.next()?;
    Some(it.fold(first, |acc, c| ar.bor(acc, c)))
}

/// Symbolically execute the function: args are Term::Arg vars.
pub fn execute(body: &FuncBody, unroll_k: u32, ar: &mut Arena) -> Result<SymResult, ()> {
    let mut locals: Vec<TId> = body
        .params
        .iter()
        .enumerate()
        .map(|(i, t)| ar.add(Term::Arg { i: i as u8, w: w_of(*t) }))
        .collect();
    for t in &body.locals {
        locals.push(ar.c(w_of(*t), 0));
    }
    let sim = Sim {
        ops: &body.ops,
        unroll_k,
        n_results: body.n_results,
    };
    let tru = ar.c(1, 1);
    let st = State { locals, stack: Vec::new() };
    let out = sim.region(0, st, tru, ar)?;
    if out.was_else || out.after != body.ops.len() || !out.exits.is_empty() {
        return Err(());
    }

    // Merge function results: fall-through stack top + early returns.
    let mut result_arrivals: Vec<(TId, TId)> = Vec::new(); // (cond, value)
    if let Some((c, s)) = &out.fall {
        if body.n_results == 1 {
            result_arrivals.push((*c, *s.stack.last().ok_or(())?));
        }
    }
    for (c, vals) in &out.rets {
        if body.n_results == 1 {
            result_arrivals.push((*c, *vals.first().ok_or(())?));
        }
    }
    let result = if body.n_results == 1 {
        let mut it = result_arrivals.into_iter();
        let (_, first) = it.next().ok_or(())?;
        Some(it.fold(first, |acc, (c, v)| {
            if acc == v {
                acc
            } else {
                let w = ar.width(acc);
                ar.add(Term::Ite { c, a: v, b: acc, w })
            }
        }))
    } else if body.n_results == 0 {
        None
    } else {
        return Err(());
    };

    Ok(SymResult {
        result,
        trap: or_all(&out.traps, ar),
        incomplete: or_all(&out.incompletes, ar),
        unroll_k,
    })
}
