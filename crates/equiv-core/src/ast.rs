//! The eqc/0 contract AST: one struct per spec §2, one enum arm per spec §3
//! opcode. The closed opcode set is the frozen surface; adding an arm here is
//! a format-version event, not a refactor.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    V128,
}

impl ValType {
    pub fn code(self) -> u64 {
        match self {
            ValType::I32 => 0,
            ValType::I64 => 1,
            ValType::F32 => 2,
            ValType::F64 => 3,
            ValType::V128 => 4,
        }
    }

    pub fn from_code(c: u64) -> Option<Self> {
        Some(match c {
            0 => ValType::I32,
            1 => ValType::I64,
            2 => ValType::F32,
            3 => ValType::F64,
            4 => ValType::V128,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssumeFlag {
    NoImports,
    NoIndirect,
    NoMemoryGrow,
    NoTrap,
    IntrinsicsAlloc,
    IntrinsicsBulkMemory,
    IntrinsicsPanicAbort,
}

impl AssumeFlag {
    pub fn code(self) -> u64 {
        match self {
            AssumeFlag::NoImports => 0,
            AssumeFlag::NoIndirect => 1,
            AssumeFlag::NoMemoryGrow => 2,
            AssumeFlag::NoTrap => 3,
            AssumeFlag::IntrinsicsAlloc => 4,
            AssumeFlag::IntrinsicsBulkMemory => 5,
            AssumeFlag::IntrinsicsPanicAbort => 6,
        }
    }

    pub fn from_code(c: u64) -> Option<Self> {
        Some(match c {
            0 => AssumeFlag::NoImports,
            1 => AssumeFlag::NoIndirect,
            2 => AssumeFlag::NoMemoryGrow,
            3 => AssumeFlag::NoTrap,
            4 => AssumeFlag::IntrinsicsAlloc,
            5 => AssumeFlag::IntrinsicsBulkMemory,
            6 => AssumeFlag::IntrinsicsPanicAbort,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BvOp {
    Add,
    Sub,
    Mul,
    DivU,
    DivS,
    RemU,
    RemS,
    And,
    Or,
    Xor,
    Shl,
    ShrU,
    ShrS,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    Ne,
    LtU,
    LtS,
    LeU,
    LeS,
    GtU,
    GtS,
    GeU,
    GeS,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    C32(u32),
    C64(u64),
    /// Reference to the target's n-th parameter.
    Arg(u8),
    /// The target's (first) result.
    Result,
    /// The bound variable of the innermost forall_b.
    Idx,
    Bv(BvOp, Box<Expr>, Box<Expr>),
    Zext(Box<Expr>),
    Wrap(Box<Expr>),
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    Band(Box<Expr>, Box<Expr>),
    Bor(Box<Expr>, Box<Expr>),
    Bnot(Box<Expr>),
    Implies(Box<Expr>, Box<Expr>),
    Ite(Box<Expr>, Box<Expr>, Box<Expr>),
    Byte(State, Box<Expr>),
    Word32(State, Box<Expr>),
    Word64(State, Box<Expr>),
    Range(Box<Expr>, Box<Expr>),
    Readable(Box<Expr>),
    Writable(Box<Expr>),
    Disjoint(Box<Expr>, Box<Expr>),
    MemEq(State, Box<Expr>, State, Box<Expr>),
    Unchanged(Box<Expr>),
    /// forall_b(K, N, body): defined as the finite conjunction over
    /// idx = 0..K-1 of (idx < N -> body). K is a static literal <= 65536.
    ForallB(u32, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refines {
    pub artifact_sha256: [u8; 32],
    pub func: String,
    pub over: Vec<Expr>,
    pub assuming: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// Format version; 0 for eqc/0-draft.
    pub version: u8,
    pub target: Target,
    /// Sorted, de-duplicated.
    pub assume: Vec<AssumeFlag>,
    pub pre: Expr,
    pub post: Expr,
    /// Region expressions the target may write; empty = writes nothing.
    pub frame: Vec<Expr>,
    pub refines: Option<Refines>,
    /// Feature bitset; must be 0 in eqc/0. Unknown bit => reject.
    pub features: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidateError {
    NestedForall,
    ForallBoundTooLarge(u32),
    IdxOutsideForall,
    ArgOutOfRange(u8),
}

impl Contract {
    /// Structural validation per spec §3/§4: forall_b nesting and bound
    /// limits, idx scoping, arg indices within the target signature.
    pub fn validate(&self) -> Result<(), ValidateError> {
        let nparams = self.target.params.len() as u8;
        let mut exprs: Vec<&Expr> = vec![&self.pre, &self.post];
        exprs.extend(self.frame.iter());
        if let Some(r) = &self.refines {
            exprs.extend(r.over.iter());
            if let Some(a) = &r.assuming {
                exprs.push(a);
            }
        }
        for e in exprs {
            walk(e, false, nparams)?;
        }
        Ok(())
    }
}

fn walk(e: &Expr, in_forall: bool, nparams: u8) -> Result<(), ValidateError> {
    match e {
        Expr::Idx if !in_forall => Err(ValidateError::IdxOutsideForall),
        Expr::Idx => Ok(()),
        Expr::Arg(n) => {
            if *n >= nparams {
                Err(ValidateError::ArgOutOfRange(*n))
            } else {
                Ok(())
            }
        }
        Expr::C32(_) | Expr::C64(_) | Expr::Result => Ok(()),
        Expr::ForallB(k, n, body) => {
            if in_forall {
                return Err(ValidateError::NestedForall);
            }
            if *k > 65536 {
                return Err(ValidateError::ForallBoundTooLarge(*k));
            }
            walk(n, in_forall, nparams)?;
            walk(body, true, nparams)
        }
        Expr::Bv(_, a, b)
        | Expr::Cmp(_, a, b)
        | Expr::Band(a, b)
        | Expr::Bor(a, b)
        | Expr::Implies(a, b)
        | Expr::Range(a, b)
        | Expr::Disjoint(a, b) => {
            walk(a, in_forall, nparams)?;
            walk(b, in_forall, nparams)
        }
        Expr::MemEq(_, a, _, b) => {
            walk(a, in_forall, nparams)?;
            walk(b, in_forall, nparams)
        }
        Expr::Ite(c, a, b) => {
            walk(c, in_forall, nparams)?;
            walk(a, in_forall, nparams)?;
            walk(b, in_forall, nparams)
        }
        Expr::Zext(a)
        | Expr::Wrap(a)
        | Expr::Bnot(a)
        | Expr::Byte(_, a)
        | Expr::Word32(_, a)
        | Expr::Word64(_, a)
        | Expr::Readable(a)
        | Expr::Writable(a)
        | Expr::Unchanged(a) => walk(a, in_forall, nparams),
    }
}
