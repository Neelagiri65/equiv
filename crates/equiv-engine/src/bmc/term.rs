//! Hash-consed bitvector term DAG. Both the symbolic executor (WASM side)
//! and the contract translator build terms here; the blaster lowers them to
//! CNF once, shared subterms shared.

use equiv_core::ast::{BvOp, CmpOp};
use std::collections::HashMap;

pub type TId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    /// `w`-bit constant (value in low bits).
    Const { w: u8, v: u64 },
    /// The i-th function argument, `w` bits.
    Arg { i: u8, w: u8 },
    /// Binary bitvector op, both operands `w` bits. Div/rem never appear
    /// (excluded at suitability time).
    Bin { op: BvOp, w: u8, a: TId, b: TId },
    /// Comparison; result is 1 bit.
    Cmp { op: CmpOp, a: TId, b: TId },
    /// 1 iff operand is non-zero (1-bit result).
    NeZero { a: TId },
    /// Bitwise not (any width).
    Not { a: TId },
    /// c is 1-bit; a, b are `w` bits.
    Ite { c: TId, a: TId, b: TId, w: u8 },
    /// Zero/sign extension to `w` bits.
    Ext { a: TId, w: u8, signed: bool },
    /// Truncation to `w` low bits.
    Slice { a: TId, w: u8 },
}

#[derive(Default)]
pub struct Arena {
    nodes: Vec<Term>,
    dedup: HashMap<Term, TId>,
}

impl Arena {
    pub fn add(&mut self, t: Term) -> TId {
        if let Some(id) = self.dedup.get(&t) {
            return *id;
        }
        let id = self.nodes.len() as TId;
        self.nodes.push(t.clone());
        self.dedup.insert(t, id);
        id
    }

    pub fn node(&self, id: TId) -> &Term {
        &self.nodes[id as usize]
    }

    pub fn width(&self, id: TId) -> u8 {
        match self.node(id) {
            Term::Const { w, .. }
            | Term::Arg { w, .. }
            | Term::Bin { w, .. }
            | Term::Ite { w, .. }
            | Term::Ext { w, .. }
            | Term::Slice { w, .. } => *w,
            Term::Cmp { .. } | Term::NeZero { .. } => 1,
            Term::Not { a } => self.width(*a),
        }
    }

    // Convenience constructors.
    pub fn c(&mut self, w: u8, v: u64) -> TId {
        let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
        self.add(Term::Const { w, v: v & mask })
    }

    pub fn band(&mut self, a: TId, b: TId) -> TId {
        self.add(Term::Bin { op: BvOp::And, w: 1, a, b })
    }

    pub fn bor(&mut self, a: TId, b: TId) -> TId {
        self.add(Term::Bin { op: BvOp::Or, w: 1, a, b })
    }

    pub fn bnot(&mut self, a: TId) -> TId {
        self.add(Term::Not { a })
    }
}
