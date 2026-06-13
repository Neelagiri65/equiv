//! Bit-blasting: term DAG -> CNF via Tseitin encoding, solved with varisat
//! (pure Rust => single static binary, deterministic single-threaded search).
//! Clause count doubles as the deterministic "solver steps" budget metric.

use super::term::{Arena, TId, Term};
use equiv_core::ast::{BvOp, CmpOp};
use std::collections::HashMap;
use varisat::{ExtendFormula, Lit, Solver};

pub struct Blaster<'a> {
    solver: Solver<'a>,
    cache: HashMap<TId, Vec<Lit>>,
    tru: Lit,
    pub clauses: u64,
}

impl<'a> Blaster<'a> {
    pub fn new() -> Self {
        let mut solver = Solver::new();
        let tru = solver.new_lit();
        let mut b = Blaster {
            solver,
            cache: HashMap::new(),
            tru,
            clauses: 0,
        };
        b.clause(&[tru]);
        b
    }

    fn clause(&mut self, lits: &[Lit]) {
        self.solver.add_clause(lits);
        self.clauses += 1;
    }

    fn lit_const(&self, v: bool) -> Lit {
        if v {
            self.tru
        } else {
            !self.tru
        }
    }

    fn gate_and(&mut self, a: Lit, b: Lit) -> Lit {
        let o = self.solver.new_lit();
        self.clause(&[!a, !b, o]);
        self.clause(&[a, !o]);
        self.clause(&[b, !o]);
        o
    }

    fn gate_or(&mut self, a: Lit, b: Lit) -> Lit {
        let o = self.gate_and(!a, !b);
        !o
    }

    fn gate_xor(&mut self, a: Lit, b: Lit) -> Lit {
        let o = self.solver.new_lit();
        self.clause(&[!a, !b, !o]);
        self.clause(&[a, b, !o]);
        self.clause(&[!a, b, o]);
        self.clause(&[a, !b, o]);
        o
    }

    fn gate_ite(&mut self, c: Lit, t: Lit, e: Lit) -> Lit {
        let o = self.solver.new_lit();
        self.clause(&[!c, !t, o]);
        self.clause(&[!c, t, !o]);
        self.clause(&[c, !e, o]);
        self.clause(&[c, e, !o]);
        o
    }

    /// (sum, carry_out)
    fn full_adder(&mut self, a: Lit, b: Lit, cin: Lit) -> (Lit, Lit) {
        let axb = self.gate_xor(a, b);
        let s = self.gate_xor(axb, cin);
        let t1 = self.gate_and(a, b);
        let t2 = self.gate_and(cin, axb);
        let cout = self.gate_or(t1, t2);
        (s, cout)
    }

    /// a + b + cin, returning (bits, carry_out).
    fn adder(&mut self, a: &[Lit], b: &[Lit], mut carry: Lit) -> (Vec<Lit>, Lit) {
        let mut out = Vec::with_capacity(a.len());
        for (x, y) in a.iter().zip(b) {
            let (s, c) = self.full_adder(*x, *y, carry);
            out.push(s);
            carry = c;
        }
        (out, carry)
    }

    /// Unsigned a < b  ==  NOT carry_out(a + ~b + 1).
    fn ult(&mut self, a: &[Lit], b: &[Lit]) -> Lit {
        let nb: Vec<Lit> = b.iter().map(|l| !*l).collect();
        let one = self.lit_const(true);
        let (_, cout) = self.adder(a, &nb, one);
        !cout
    }

    fn slt(&mut self, a: &[Lit], b: &[Lit]) -> Lit {
        // Flip sign bits, compare unsigned.
        let mut a2 = a.to_vec();
        let mut b2 = b.to_vec();
        let msb = a.len() - 1;
        a2[msb] = !a2[msb];
        b2[msb] = !b2[msb];
        self.ult(&a2, &b2)
    }

    fn eq(&mut self, a: &[Lit], b: &[Lit]) -> Lit {
        let mut acc = self.lit_const(true);
        for (x, y) in a.iter().zip(b) {
            let d = self.gate_xor(*x, *y);
            acc = self.gate_and(acc, !d);
        }
        acc
    }

    fn or_reduce(&mut self, a: &[Lit]) -> Lit {
        let mut acc = self.lit_const(false);
        for x in a {
            acc = self.gate_or(acc, *x);
        }
        acc
    }

    /// Barrel shifter. `amt` uses only its low log2(w) bits (WASM mod-width
    /// shift semantics). `fill` provides the shifted-in bit (per-step for
    /// shr_s, where it is the current sign bit).
    fn shift(&mut self, a: &[Lit], amt: &[Lit], left: bool, arith: bool) -> Vec<Lit> {
        let w = a.len();
        let stages = w.trailing_zeros() as usize; // w is 32 or 64
        let mut cur = a.to_vec();
        for stage in 0..stages {
            let dist = 1usize << stage;
            let sel = amt[stage];
            let fill = if arith {
                cur[w - 1]
            } else {
                self.lit_const(false)
            };
            let mut next = Vec::with_capacity(w);
            for i in 0..w {
                let shifted = if left {
                    if i >= dist {
                        cur[i - dist]
                    } else {
                        self.lit_const(false)
                    }
                } else if i + dist < w {
                    cur[i + dist]
                } else {
                    fill
                };
                next.push(self.gate_ite(sel, shifted, cur[i]));
            }
            cur = next;
        }
        cur
    }

    /// Shift-and-add multiplier.
    fn mul(&mut self, a: &[Lit], b: &[Lit]) -> Vec<Lit> {
        let w = a.len();
        let mut acc = vec![self.lit_const(false); w];
        for (i, bi) in b.iter().enumerate() {
            // addend = (a << i) masked by b_i
            let mut addend = Vec::with_capacity(w);
            for j in 0..w {
                if j < i {
                    addend.push(self.lit_const(false));
                } else {
                    addend.push(self.gate_and(a[j - i], *bi));
                }
            }
            let zero = self.lit_const(false);
            let (sum, _) = self.adder(&acc, &addend, zero);
            acc = sum;
        }
        acc
    }

    pub fn fresh_vec(&mut self, w: u8) -> Vec<Lit> {
        (0..w).map(|_| self.solver.new_lit()).collect()
    }

    /// Lower a term to its bit vector. `args` maps Arg index -> bits.
    pub fn blast(&mut self, arena: &Arena, id: TId, args: &[Vec<Lit>]) -> Vec<Lit> {
        if let Some(bits) = self.cache.get(&id) {
            return bits.clone();
        }
        let bits = match arena.node(id).clone() {
            Term::Const { w, v } => (0..w).map(|i| self.lit_const((v >> i) & 1 == 1)).collect(),
            Term::Arg { i, .. } => args[i as usize].clone(),
            Term::Not { a } => self.blast(arena, a, args).iter().map(|l| !*l).collect(),
            Term::Ext { a, w, signed } => {
                let av = self.blast(arena, a, args);
                let fill = if signed {
                    *av.last().unwrap()
                } else {
                    self.lit_const(false)
                };
                let mut out = av.clone();
                out.resize(w as usize, fill);
                out
            }
            Term::Slice { a, w } => {
                let av = self.blast(arena, a, args);
                av[..w as usize].to_vec()
            }
            Term::NeZero { a } => {
                let av = self.blast(arena, a, args);
                vec![self.or_reduce(&av)]
            }
            Term::Ite { c, a, b, .. } => {
                let cv = self.blast(arena, c, args)[0];
                let av = self.blast(arena, a, args);
                let bv = self.blast(arena, b, args);
                av.iter()
                    .zip(&bv)
                    .map(|(t, e)| self.gate_ite(cv, *t, *e))
                    .collect()
            }
            Term::Cmp { op, a, b } => {
                let av = self.blast(arena, a, args);
                let bv = self.blast(arena, b, args);
                let bit = match op {
                    CmpOp::Eq => self.eq(&av, &bv),
                    CmpOp::Ne => {
                        let e = self.eq(&av, &bv);
                        !e
                    }
                    CmpOp::LtU => self.ult(&av, &bv),
                    CmpOp::LtS => self.slt(&av, &bv),
                    CmpOp::GtU => self.ult(&bv, &av),
                    CmpOp::GtS => self.slt(&bv, &av),
                    CmpOp::LeU => !self.ult(&bv, &av),
                    CmpOp::LeS => !self.slt(&bv, &av),
                    CmpOp::GeU => !self.ult(&av, &bv),
                    CmpOp::GeS => !self.slt(&av, &bv),
                };
                vec![bit]
            }
            Term::Bin { op, a, b, .. } => {
                let av = self.blast(arena, a, args);
                let bv = self.blast(arena, b, args);
                match op {
                    BvOp::Add => {
                        let zero = self.lit_const(false);
                        self.adder(&av, &bv, zero).0
                    }
                    BvOp::Sub => {
                        let nb: Vec<Lit> = bv.iter().map(|l| !*l).collect();
                        let one = self.lit_const(true);
                        self.adder(&av, &nb, one).0
                    }
                    BvOp::Mul => self.mul(&av, &bv),
                    BvOp::And => av
                        .iter()
                        .zip(&bv)
                        .map(|(x, y)| self.gate_and(*x, *y))
                        .collect(),
                    BvOp::Or => av
                        .iter()
                        .zip(&bv)
                        .map(|(x, y)| self.gate_or(*x, *y))
                        .collect(),
                    BvOp::Xor => av
                        .iter()
                        .zip(&bv)
                        .map(|(x, y)| self.gate_xor(*x, *y))
                        .collect(),
                    BvOp::Shl => self.shift(&av, &bv, true, false),
                    BvOp::ShrU => self.shift(&av, &bv, false, false),
                    BvOp::ShrS => self.shift(&av, &bv, false, true),
                    BvOp::DivU | BvOp::DivS | BvOp::RemU | BvOp::RemS => {
                        unreachable!("div/rem excluded at suitability time")
                    }
                }
            }
        };
        self.cache.insert(id, bits.clone());
        bits
    }

    /// Assert a 1-bit term true, solve, and on SAT return the argument
    /// values from the model.
    pub fn solve_assert(
        &mut self,
        arena: &Arena,
        formula: TId,
        args: &[Vec<Lit>],
    ) -> Result<Option<Vec<u64>>, String> {
        let bits = self.blast(arena, formula, args);
        let f = bits[0];
        self.clause(&[f]);
        let sat = self.solver.solve().map_err(|e| format!("{e:?}"))?;
        if !sat {
            return Ok(None);
        }
        let model = self.solver.model().ok_or("sat but no model")?;
        let truth: std::collections::HashSet<Lit> = model.into_iter().collect();
        let vals = args
            .iter()
            .map(|bits| {
                bits.iter()
                    .enumerate()
                    .map(|(i, l)| if truth.contains(l) { 1u64 << i } else { 0 })
                    .sum()
            })
            .collect();
        Ok(Some(vals))
    }
}
