//! The review oracle: deterministic differential review of an AI-written
//! source function against a reference implementation.
//!
//! The review question this answers, the one that matters when an AI
//! rewrites or optimises a function: is the new version behaviourally
//! equivalent to the reference, or here is the exact input where it
//! diverges. The answer is a reproducible, signed receipt.
//!
//! Determinism: input generation and the
//! verdict are computed in Rust from a fixed seed. The language runtime
//! (here: python3) is a dumb evaluator. It never decides anything that
//! reaches the receipt, so the receipt is reproducible regardless of runtime
//! flakiness. Honest scope: integer / string / list-of-int I/O, where value
//! reprs are stable and identical across hosts, plus float64 inside the
//! IEEE-754 correctly-rounded operation set (see spec-type-admissibility.md).
//! A function that reaches a transcendental is refused by name, since its last
//! bit is not reproducible across maths libraries. Objects and any value with
//! nondeterministic ordering are out of this slice on purpose.

use equiv_core::cbor::{self, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgType {
    Int,
    Str,
    ListInt,
    Float,
    /// A JSON-structural map with string keys and admissible scalar values.
    Dict,
}

impl ArgType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "int" => ArgType::Int,
            "str" => ArgType::Str,
            "list[int]" | "list" => ArgType::ListInt,
            "float" => ArgType::Float,
            "dict" | "dict[str,int]" => ArgType::Dict,
            _ => return None,
        })
    }
}

/// Default step budget per call: how many Python line and jump events one call
/// of the candidate or the reference may raise before the review stops and
/// reports could-not-check. Counted events, never wall-clock time, so the same
/// budget gives the same verdict on every host (gate P3).
pub const DEFAULT_BUDGET: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct ReviewSpec {
    pub func: String,
    pub args: Vec<ArgType>,
    pub n: u32,
    pub seed: u64,
    /// Step budget per call (and per module load), in Python line and jump
    /// events (sys.monitoring LINE | JUMP). Bound into the spec hash and
    /// written into the receipt.
    pub budget: u64,
}

impl ReviewSpec {
    fn canonical(&self) -> Vec<u8> {
        // Stable bytes for the spec, so the receipt binds exactly what was
        // checked (function, signature, case count, seed).
        let mut v = Vec::new();
        v.extend_from_slice(self.func.as_bytes());
        v.push(0);
        for a in &self.args {
            v.push(match a {
                ArgType::Int => 1,
                ArgType::Str => 2,
                ArgType::ListInt => 3,
                ArgType::Float => 4,
                ArgType::Dict => 5,
            });
        }
        v.extend_from_slice(&self.n.to_le_bytes());
        v.extend_from_slice(&self.seed.to_le_bytes());
        v.extend_from_slice(&self.budget.to_le_bytes());
        v
    }
}

// ---------- deterministic input generation (Rust-side, seeded) ----------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = (hi - lo + 1) as u64;
        lo + (self.next() % span) as i64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyVal {
    Int(i64),
    Str(String),
    ListInt(Vec<i64>),
    /// float64 stored as its raw IEEE-754 bits (keeps Eq and exactness; f64
    /// itself is not Eq). Generated in Rust, handed to the evaluator exactly.
    Float(u64),
    /// A string-keyed map of ints (the first JSON-structural object type).
    Dict(Vec<(String, i64)>),
}

impl PyVal {
    /// A Python literal: what we hand the evaluator. Floats are reconstructed
    /// from exact bits via struct (no decimal parse means no rounding on the
    /// way in); the driver imports `struct`.
    fn to_py(&self) -> String {
        match self {
            PyVal::Int(n) => n.to_string(),
            PyVal::Str(s) => py_str_literal(s),
            PyVal::ListInt(xs) => {
                let inner: Vec<String> = xs.iter().map(|n| n.to_string()).collect();
                format!("[{}]", inner.join(", "))
            }
            PyVal::Float(bits) => {
                format!("struct.unpack('>d', bytes.fromhex('{bits:016x}'))[0]")
            }
            PyVal::Dict(pairs) => {
                let inner: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {v}", py_str_literal(k))).collect();
                format!("{{{}}}", inner.join(", "))
            }
        }
    }
    /// Human readable and stable: what a counterexample shows the reviewer.
    pub fn display(&self) -> String {
        match self {
            PyVal::Float(bits) => {
                let f = f64::from_bits(*bits);
                if f.is_nan() {
                    "nan".to_string()
                } else {
                    format!("{f:?}")
                }
            }
            _ => self.to_py(),
        }
    }
}

// A Python string literal for any Unicode string. Printable ASCII is emitted
// as-is (so existing ASCII cases and receipts are byte-for-byte unchanged);
// everything else uses an explicit escape, so tabs, newlines and non-ASCII
// characters reach the evaluator exactly and the literal is the same on
// every host.
fn py_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            ' '..='~' => out.push(ch),
            c if (c as u32) <= 0xFF => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xFFFF => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('\'');
    out
}

fn gen_arg(rng: &mut Rng, ty: ArgType) -> PyVal {
    match ty {
        // Spread includes 0, negatives and small positives so off-by-one
        // and sign bugs surface fast.
        // Magnitudes stay review-realistic: a reference may be O(n), so we
        // must not hand it astronomically large inputs that OOM or hang.
        ArgType::Int => PyVal::Int(match rng.next() % 5 {
            0 => 0,
            1 => rng.range(1, 12),
            2 => rng.range(-12, -1),
            3 => rng.range(-1000, 1000),
            _ => rng.range(-1_000_000, 1_000_000),
        }),
        ArgType::Str => {
            let len = (rng.next() % 9) as usize;
            let s: String = (0..len)
                .map(|_| (b'a' + (rng.next() % 26) as u8) as char)
                .collect();
            PyVal::Str(s)
        }
        ArgType::ListInt => {
            let len = (rng.next() % 7) as usize;
            PyVal::ListInt((0..len).map(|_| rng.range(-20, 20)).collect())
        }
        // Cover the corners where refactors break: signed zero, infinities,
        // NaN, subnormals, largest finite, powers of two across the exponent
        // range, mixed magnitudes (so reassociation/overflow differences are
        // reachable). Typical finite values with a fraction too.
        ArgType::Float => PyVal::Float(match rng.next() % 14 {
            0 => 0x0000_0000_0000_0000,  // +0.0
            1 => 0x8000_0000_0000_0000,  // -0.0
            2 => 0x7FF0_0000_0000_0000,  // +inf
            3 => 0xFFF0_0000_0000_0000,  // -inf
            4 => 0x7FF8_0000_0000_0000,  // canonical quiet NaN
            5 => 0x7FF8_0000_0000_0001,  // NaN with a different payload
            6 => 0x0000_0000_0000_0001,  // smallest positive subnormal
            7 => 0x000F_FFFF_FFFF_FFFF,  // largest subnormal
            8 => 0x7FEF_FFFF_FFFF_FFFF,  // largest finite (f64::MAX)
            9 => 0x0010_0000_0000_0000,  // smallest positive normal
            10 => {
                // a power of two across a wide exponent range
                let k = rng.range(-60, 60) as i32;
                2f64.powi(k).to_bits()
            }
            11 => rng.next(), // a fully random bit pattern (any class)
            12 => {
                // small magnitude with a fraction
                let n = rng.range(-1000, 1000) as f64;
                let frac = (rng.next() % 1000) as f64 / 1000.0;
                (n + frac).to_bits()
            }
            _ => {
                // typical, review-realistic magnitude with a fraction
                let n = rng.range(-1_000_000, 1_000_000) as f64;
                let frac = (rng.next() % 1_000_000) as f64 / 1_000_000.0;
                (n + frac).to_bits()
            }
        }),
        // A small string-keyed map of ints. Keys are a deduplicated subset of
        // a..h so insertion order varies but content is well defined.
        ArgType::Dict => {
            let len = (rng.next() % 5) as usize;
            let mut keys = std::collections::BTreeSet::new();
            let mut pairs = Vec::new();
            for _ in 0..len {
                let k = ((b'a' + (rng.next() % 8) as u8) as char).to_string();
                if keys.insert(k.clone()) {
                    pairs.push((k, rng.range(-20, 20)));
                }
            }
            PyVal::Dict(pairs)
        }
    }
}

// Magnitude envelope for synthesised integer inputs. A reference may be O(n),
// so inputs must not be astronomically large (would hang or OOM). The same
// bound the random generator respects, applied to boundary and literal inputs.
const INT_ENVELOPE: i64 = 1_000_000;

// A neutral, non-degenerate value per type: held constant for the other
// arguments while one argument is varied across its corners (one-at-a-time
// enumeration keeps the case count linear, not a cartesian explosion).
fn default_val(ty: ArgType) -> PyVal {
    match ty {
        ArgType::Int => PyVal::Int(0),
        ArgType::Str => PyVal::Str(String::new()),
        ArgType::ListInt => PyVal::ListInt(Vec::new()),
        ArgType::Float => PyVal::Float(0x3FF0_0000_0000_0000), // 1.0
        ArgType::Dict => PyVal::Dict(Vec::new()),
    }
}

// String corners the random generator (a-z, len<=8) cannot reach. Grouped by
// the semantics they stress; each is a well-known place where string
// refactors change behaviour. Fixed and ordered, so the case list is the same
// on every host. (A lone surrogate cannot be held in a Rust String, so that
// class stays out of reach; see divergences/README.md.)
const STR_CORNERS: &[&str] = &[
    // shape: empty, case, digit, space, longer than the random range
    "", "a", "A", "0", " ", "abcdefghij",
    // whitespace other than a single space
    "\t", "\n", "\tx\n", " x ", "\r\n", "\u{0b}\u{0c}", "\u{a0}", "\u{2003}", "\u{3000}",
    // case mapping that differs between lower() and casefold()/upper()
    "Straße", "ß", "İ", "ς", "ﬁ",
    // digits outside ASCII: isdigit/isdecimal/isnumeric disagree
    "²", "٣", "７",
    // multi-byte, combining and astral characters: char vs byte length
    "éééééé", "e\u{301}", "😀",
    // digit strings in identifier shape (check digits: Luhn, ISBN, IBAN-like)
    "91", "0123456789", "79927398713", "4111111111111111", "123456789X", "0306406152",
];

// Deterministic corner inputs the seeded random generator reaches rarely or
// never: the exact shapes where refactors break. All stay inside the O(n)-safe
// envelope so a linear reference cannot hang on them.
fn boundary_cases(ty: ArgType) -> Vec<PyVal> {
    match ty {
        // Off-by-one and sign corners (0, +/-1, +/-2) within the envelope.
        ArgType::Int => [0, 1, -1, 2, -2].into_iter().map(PyVal::Int).collect(),
        // Shapes the random generator (a-z, len<=8) cannot reach: empty,
        // uppercase, digit, whitespace, and a string longer than 8.
        ArgType::Str => STR_CORNERS.iter().map(|s| PyVal::Str(s.to_string())).collect(),
        // Structural corners the random generator (len<=6, vals -20..20) misses:
        // empty, singleton, all-equal, sorted, reverse-sorted, length>6.
        ArgType::ListInt => [
            vec![],
            vec![0],
            vec![1, 1, 1],
            vec![1, 2, 3],
            vec![3, 2, 1],
            (0..10).collect::<Vec<i64>>(),
            // Digit lists in check-digit shape: an even-length list that is a
            // valid Luhn number only when read from the right (so position-
            // parity bugs diverge), and a 16-digit card-length list.
            vec![1, 8],
            vec![4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
        ]
        .into_iter()
        .map(PyVal::ListInt)
        .collect(),
        // Guarantee the corner bit patterns are ALWAYS tested (the random
        // generator only reaches them probabilistically per run).
        ArgType::Float => [
            0x0000_0000_0000_0000u64, // +0.0
            0x8000_0000_0000_0000,    // -0.0
            0x3FF0_0000_0000_0000,    // 1.0
            0xBFF0_0000_0000_0000,    // -1.0
            0x7FF0_0000_0000_0000,    // +inf
            0xFFF0_0000_0000_0000,    // -inf
            0x7FF8_0000_0000_0000,    // quiet NaN
            0x0000_0000_0000_0001,    // smallest subnormal
            0x7FEF_FFFF_FFFF_FFFF,    // largest finite
        ]
        .into_iter()
        .map(PyVal::Float)
        .collect(),
        ArgType::Dict => vec![
            PyVal::Dict(vec![]),
            PyVal::Dict(vec![("a".into(), 0)]),
            PyVal::Dict(vec![("a".into(), 1), ("b".into(), 2)]),
        ],
    }
}

// ---- conservative, deterministic literal scan over the source ----
//
// NOT a full parse: a token scan over the combined candidate+reference text.
// It is a SUPERSET (it may pick up a literal from a comment or an unrelated
// line), which only ever adds harmless extra inputs, never removes coverage.
// The discriminating input usually sits on a branch condition, and the branch
// constants live in the source, so we aim inputs at and around them.

// Literal-derived ints may exceed INT_ENVELOPE: they are the source's own
// branch constants (masks such as 0xFFFFFFFF, limits such as 2**31), so the
// function under review already expects values of that size. Capped so L+1
// cannot overflow.
const LITERAL_INT_CAP: i64 = i64::MAX - 1;

// One integer literal token: decimal, 0x/0o/0b, with optional underscores.
// Returns (value, end index). Floats (1.5, 1e3) and identifiers (x1) are not
// integer literals and return None.
fn int_token(b: &[u8], start: usize) -> Option<(i64, usize)> {
    let prev = if start > 0 { b[start - 1] } else { b' ' };
    if prev == b'_' || prev.is_ascii_alphanumeric() || prev == b'.' {
        return None;
    }
    let (radix, digits_from) = match (b.get(start), b.get(start + 1).map(|c| c.to_ascii_lowercase())) {
        (Some(b'0'), Some(b'x')) => (16, start + 2),
        (Some(b'0'), Some(b'o')) => (8, start + 2),
        (Some(b'0'), Some(b'b')) => (2, start + 2),
        _ => (10, start),
    };
    let mut i = digits_from;
    let mut text = String::new();
    while i < b.len() && (b[i] == b'_' || (b[i] as char).is_digit(radix)) {
        if b[i] != b'_' {
            text.push(b[i] as char);
        }
        i += 1;
    }
    let next = if i < b.len() { b[i] } else { b' ' };
    if text.is_empty() || next == b'.' || next == b'_' || next.is_ascii_alphanumeric() {
        return None;
    }
    i64::from_str_radix(&text, radix).ok().map(|v| (v, i))
}

// Integer literals in the source, plus simple constant expressions of two
// literals (`1 << 19`, `2 ** 31`, `60 * 60`) folded to their value: a branch
// constant is often written as an expression, not a bare number. A token
// scan, not an evaluator: only literal-op-literal with checked arithmetic.
fn scan_int_literals(src: &str) -> Vec<i64> {
    let b = src.as_bytes();
    let mut toks: Vec<(i64, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            match int_token(b, i) {
                Some((v, end)) => {
                    toks.push((v, i, end));
                    i = end;
                }
                None => {
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                        i += 1;
                    }
                }
            }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let mut out: Vec<i64> = toks.iter().map(|t| t.0).collect();
    for w in toks.windows(2) {
        let (a, _, a_end) = w[0];
        let (c, c_start, _) = w[1];
        let folded = match src[a_end..c_start].trim() {
            "<<" if (0..63).contains(&c) => a.checked_shl(c as u32).filter(|v| v >> c == a),
            "**" if (0..64).contains(&c) => a.checked_pow(c as u32),
            "*" => a.checked_mul(c),
            _ => None,
        };
        out.extend(folded);
    }
    out.retain(|n| n.abs() <= LITERAL_INT_CAP);
    out
}

fn scan_float_literals(src: &str) -> Vec<f64> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                i += 1;
                let frac = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i > frac {
                    if let Ok(f) = src[start..i].parse::<f64>() {
                        if f.is_finite() && f.abs() <= INT_ENVELOPE as f64 {
                            out.push(f);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn scan_str_literals(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let q = b[i];
        if q == b'\'' || q == b'"' {
            let mut j = i + 1;
            let mut s = String::new();
            let mut closed = false;
            while j < b.len() {
                if b[j] == b'\\' && j + 1 < b.len() {
                    s.push(b[j + 1] as char);
                    j += 2;
                    continue;
                }
                if b[j] == q {
                    closed = true;
                    j += 1;
                    break;
                }
                if b[j].is_ascii() {
                    s.push(b[j] as char);
                }
                j += 1;
            }
            if closed && s.len() <= 64 {
                out.push(s);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

// Inputs derived from the source's own literals, per argument type. For an int
// literal L we add L-1, L, L+1 so the exact branch value and its neighbours are
// tested. Strings and floats are added by value. (List/dict literals are not
// derived here; their boundary cases cover the structural corners.)
fn literal_inputs(src: &str, ty: ArgType) -> Vec<PyVal> {
    match ty {
        ArgType::Int => {
            let mut out = Vec::new();
            for n in scan_int_literals(src) {
                for d in [-1i64, 0, 1] {
                    if let Some(v) = n.checked_add(d) {
                        out.push(PyVal::Int(v));
                    }
                }
            }
            out
        }
        // String literals by value, plus strings whose LENGTH sits on an int
        // literal and its neighbours (a `s[:255]` or `len(s) <= 10` limit),
        // in both one-byte and two-byte characters so char-vs-byte length
        // bugs diverge at the same limit.
        ArgType::Str => {
            let mut out: Vec<PyVal> = scan_str_literals(src).into_iter().map(PyVal::Str).collect();
            for n in scan_int_literals(src) {
                if (1..=STR_LEN_CAP).contains(&n) {
                    for len in [n - 1, n, n + 1] {
                        out.push(PyVal::Str("a".repeat(len as usize)));
                        out.push(PyVal::Str("é".repeat(len as usize)));
                    }
                }
            }
            out
        }
        ArgType::Float => scan_float_literals(src)
            .into_iter()
            .map(|f| PyVal::Float(f.to_bits()))
            .collect(),
        // A dict keyed by each string literal in the source, with falsy and
        // truthy values: `d.get("port") or 8080` only diverges when the key
        // is present and its value is falsy.
        ArgType::Dict => scan_str_literals(src)
            .into_iter()
            .filter(|k| !k.is_empty())
            .flat_map(|k| [PyVal::Dict(vec![(k.clone(), 0)]), PyVal::Dict(vec![(k, 1)])])
            .collect(),
        ArgType::ListInt => Vec::new(),
    }
}

// Longest string synthesised from a length literal: covers common limits
// (255, 1024, 4096) without building huge inputs.
const STR_LEN_CAP: i64 = 4096;

// Deterministic, ordered case list: the all-default base first (simplest
// counterexample reported first), then one-at-a-time boundary + source-literal
// inputs, then the original seeded random cases. Deduplicated by Python repr so
// overlapping corners are not run twice. `sources` are the candidate and
// reference texts, scanned for branch literals. Same inputs => same cases on
// every host (AC-1); changing this generator is a versioned change (the
// receipt carries checker_version).
fn gen_cases(spec: &ReviewSpec, sources: &[&str]) -> Vec<Vec<PyVal>> {
    let mut cases: Vec<Vec<PyVal>> = Vec::new();
    let mut seen: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    fn key(c: &[PyVal]) -> Vec<String> {
        c.iter().map(|v| v.to_py()).collect()
    }

    if !spec.args.is_empty() {
        let base: Vec<PyVal> = spec.args.iter().map(|t| default_val(*t)).collect();
        if seen.insert(key(&base)) {
            cases.push(base.clone());
        }
        for (i, ty) in spec.args.iter().enumerate() {
            let mut vals = boundary_cases(*ty);
            for s in sources {
                vals.extend(literal_inputs(s, *ty));
            }
            for v in vals {
                let mut c = base.clone();
                c[i] = v;
                if seen.insert(key(&c)) {
                    cases.push(c);
                }
            }
        }
        // Paired string corners: a function comparing two strings only
        // diverges when both sides are related (same text, or one is the
        // other's case mapping, e.g. "Straße" vs "STRASSE"). One-at-a-time
        // enumeration holds the other argument at "", so it can never reach
        // these. Rust's own case mapping is compiled in, so this is the same
        // on every host.
        let str_args: Vec<usize> =
            spec.args.iter().enumerate().filter(|(_, t)| **t == ArgType::Str).map(|(i, _)| i).collect();
        for (x, &i) in str_args.iter().enumerate() {
            for &j in &str_args[x + 1..] {
                for s in STR_CORNERS {
                    for (l, r) in [
                        (s.to_string(), s.to_string()),
                        (s.to_string(), s.to_uppercase()),
                        (s.to_string(), s.to_lowercase()),
                    ] {
                        let mut c = base.clone();
                        c[i] = PyVal::Str(l);
                        c[j] = PyVal::Str(r);
                        if seen.insert(key(&c)) {
                            cases.push(c);
                        }
                    }
                }
            }
        }
    }

    let mut rng = Rng(spec.seed | 1);
    for _ in 0..spec.n {
        let c: Vec<PyVal> = spec.args.iter().map(|t| gen_arg(&mut rng, *t)).collect();
        if seen.insert(key(&c)) {
            cases.push(c);
        }
    }
    cases
}

// ---------- verdict + receipt ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewVerdict {
    /// Candidate and reference agreed on all N generated cases.
    Equivalent { n: u32 },
    /// First input where they diverge (the review's payload).
    Counterexample {
        input: String,
        candidate: String,
        reference: String,
    },
    /// The harness could not run (load error, missing function, runtime gone).
    Error { reason: String },
    /// The function is outside the admissible set (e.g. it reaches a
    /// floating-point transcendental whose last bit is not reproducible across
    /// maths libraries). No cross-host verdict is possible. Refusing by name
    /// is the feature: it is never silently mis-judged.
    Refused { reason: String },
}

impl ReviewVerdict {
    pub fn exit_code(&self) -> i32 {
        match self {
            ReviewVerdict::Equivalent { .. } => 0,
            ReviewVerdict::Counterexample { .. } => 1,
            ReviewVerdict::Error { .. } => 2,
            // Not a pass: "I will not judge this" must not read as green.
            ReviewVerdict::Refused { .. } => 2,
        }
    }
    fn value(&self) -> Value {
        match self {
            ReviewVerdict::Equivalent { n } => {
                Value::Array(vec![Value::Uint(0), Value::Uint(*n as u64)])
            }
            ReviewVerdict::Counterexample { input, candidate, reference } => Value::Array(vec![
                Value::Uint(1),
                Value::Text(input.clone()),
                Value::Text(candidate.clone()),
                Value::Text(reference.clone()),
            ]),
            ReviewVerdict::Error { reason } => {
                Value::Array(vec![Value::Uint(2), Value::Text(reason.clone())])
            }
            ReviewVerdict::Refused { reason } => {
                Value::Array(vec![Value::Uint(3), Value::Text(reason.clone())])
            }
        }
    }
}

fn sha256(b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b);
    h.finalize().into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The shared, reusable spine: a deterministic, content-addressed receipt.
/// Same inputs => byte-identical bytes (no wall-clock, no host fields). This
/// is the same determinism discipline as the WASM path. Receipts are the
/// engine-agnostic product; the verdict payload is engine specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewReceipt {
    pub candidate_sha256: [u8; 32],
    pub reference_sha256: [u8; 32],
    pub spec_sha256: [u8; 32],
    pub verdict: ReviewVerdict,
    pub checker_version: String,
    /// The step budget the review ran under (see `ReviewSpec::budget`).
    pub budget: u64,
}

impl ReviewReceipt {
    pub fn to_bytes(&self) -> Vec<u8> {
        let entries = vec![
            (0u64, Value::Uint(0)),
            (1, Value::Bytes(self.candidate_sha256.to_vec())),
            (2, Value::Bytes(self.reference_sha256.to_vec())),
            (3, Value::Bytes(self.spec_sha256.to_vec())),
            (4, self.verdict.value()),
            (
                5,
                Value::Array(vec![
                    Value::Text("equiv-review".into()),
                    Value::Text(self.checker_version.clone()),
                ]),
            ),
            (6, Value::Uint(self.budget)),
        ];
        cbor::to_bytes(&Value::Map(entries))
    }
    pub fn sha256(&self) -> [u8; 32] {
        sha256(&self.to_bytes())
    }
}

// ---------- the oracle ----------

fn driver_source(cand: &Path, refr: &Path, spec: &ReviewSpec, cases: &[Vec<PyVal>]) -> String {
    let cases_py: Vec<String> = cases
        .iter()
        .map(|c| {
            let args: Vec<String> = c.iter().map(|v| v.to_py()).collect();
            format!("({},)", args.join(", "))
        })
        .collect();
    // The evaluator decides nothing that reaches the receipt: it emits a
    // per-case status line; Rust interprets and builds the verdict. For float
    // reviews an AST allowlist (spec-type-admissibility.md AD-2) runs first, at
    // source level, before any module loads: a function is admitted only if it
    // stays inside the IEEE-754 correctly rounded closure (+ - * / and
    // math.sqrt, comparisons, literals, a small set of exact builtins). Anything
    // else (** , //, %, dynamic dispatch, unknown calls, foreign imports) is
    // refused by name. This is the allowlist the spec mandates, not a denylist.
    let admis = if spec.args.iter().any(|a| *a == ArgType::Float) {
        format!(
            r#"import ast as _ast, sys as _sys
def _bad(_path):
    try: _t = _ast.parse(open(_path, encoding='utf-8').read())
    except Exception: return 'source could not be parsed'
    _target = None
    for _n in _ast.walk(_t):
        if isinstance(_n, _ast.FunctionDef) and _n.name == {fn:?}: _target = _n; break
    if _target is None: return None
    _okcall = {{'float', 'int', 'abs', 'min', 'max', 'sum', 'len', 'range', 'round', 'bool'}}
    for _node in _ast.walk(_target):
        if isinstance(_node, _ast.BinOp) and not isinstance(_node.op, (_ast.Add, _ast.Sub, _ast.Mult, _ast.Div)):
            return 'uses operator ' + type(_node.op).__name__ + ' outside the correctly rounded closure'
        if isinstance(_node, _ast.AugAssign) and not isinstance(_node.op, (_ast.Add, _ast.Sub, _ast.Mult, _ast.Div)):
            return 'uses augmented operator ' + type(_node.op).__name__ + ' outside the closure'
        if isinstance(_node, _ast.Call):
            _f = _node.func
            if isinstance(_f, _ast.Name):
                if _f.id not in _okcall: return 'calls ' + _f.id + ' outside the admissible set'
            elif isinstance(_f, _ast.Attribute):
                if not (isinstance(_f.value, _ast.Name) and _f.value.id == 'math' and _f.attr == 'sqrt'):
                    _nm = (_f.value.id + '.' + _f.attr) if isinstance(_f.value, _ast.Name) else _f.attr
                    return 'calls ' + _nm + ' outside the admissible set'
            else:
                return 'uses a dynamic call that cannot be analysed'
        if isinstance(_node, _ast.Attribute):
            if not (isinstance(_node.value, _ast.Name) and _node.value.id == 'math' and _node.attr == 'sqrt'):
                return 'uses attribute ' + _node.attr + ' that cannot be analysed'
    return None
for _p in ({cand:?}, {refr:?}):
    _w = _bad(_p)
    if _w:
        print('__REFUSED__\t' + _w); _sys.exit(0)
"#,
            fn = spec.func,
            cand = cand,
            refr = refr,
        )
    } else {
        String::new()
    };
    format!(
        r#"import importlib.util as u
import struct
import sys as _sys
import os as _os
{admis}# Deterministic step budget (backlog 0.3, gate P3). Every LINE and JUMP event
# raised by code outside this driver file counts one step; past the budget the
# callback raises _Budget, and keeps raising on every later event, so user code
# that swallows it (even a bare except) cannot loop on. No clock is read.
if _sys.version_info < (3, 12):
    print('__NOBUDGET__\t' + _sys.version.split()[0]); _sys.exit(0)
_M = _sys.monitoring
_TOOL = 4
_EV = _M.events.LINE | _M.events.JUMP
_BUDGET = {budget}
_left = [0]
class _Budget(BaseException): pass
def _tick(code, *_a):
    if code.co_filename == __file__: return
    _left[0] -= 1
    if _left[0] < 0: raise _Budget()
_M.use_tool_id(_TOOL, 'equiv-budget')
_M.register_callback(_TOOL, _M.events.LINE, _tick)
_M.register_callback(_TOOL, _M.events.JUMP, _tick)
def _metered(fn, *args):
    # (value, exception name or None, budget exhausted?)
    _left[0] = _BUDGET; _M.set_events(_TOOL, _EV)
    try:
        v = fn(*args); e = None
    except BaseException as x:
        if _left[0] < 0 or isinstance(x, _Budget): return None, None, True
        if not isinstance(x, Exception): raise
        v = None; e = x
    finally:
        _M.set_events(_TOOL, 0)
    if _left[0] < 0: return None, None, True
    return v, e, False
def _pkg_of(_p):
    # Walk up while __init__.py exists: returns (dir to put on sys.path, dotted
    # package name). Gives the module real package context so `from . import x`
    # resolves instead of raising "no known parent package".
    _d = _os.path.dirname(_os.path.abspath(_p)); _parts = []
    while _os.path.exists(_os.path.join(_d, '__init__.py')):
        _parts.insert(0, _os.path.basename(_d)); _d = _os.path.dirname(_d)
    return _d, '.'.join(_parts)
# Package context is derived from the candidate (head) path and applied to both
# versions, so the reference (a temp file outside the tree) resolves its relative
# imports against the real package's siblings.
_root, _pkg = _pkg_of({cand:?})
if _pkg and _root not in _sys.path: _sys.path.insert(0, _root)
def load(p, n):
    fullname = (_pkg + '.' + n) if _pkg else n
    s = u.spec_from_file_location(fullname, p); m = u.module_from_spec(s)
    if _pkg: m.__package__ = _pkg
    _sys.modules[fullname] = m; s.loader.exec_module(m); return m
def canon(x):
    # Canonical, order-independent form. int/str/bool/None reproduce repr (so
    # existing reviews are unchanged); float -> exact bits (NaN -> one token);
    # list -> recursive, order preserved (order is observable); dict -> sorted
    # string keys (a map, order is not observable). Anything not JSON-structural
    # (set, tuple, complex, custom object, non-string keys) -> __NONCANON__,
    # which the driver turns into a Refused verdict.
    if isinstance(x, bool): return repr(x)
    if isinstance(x, int): return repr(x)
    if isinstance(x, float): return 'nanq' if x != x else x.hex()
    if isinstance(x, str): return repr(x)
    if x is None: return 'None'
    if isinstance(x, list): return '[' + ', '.join(canon(e) for e in x) + ']'
    if isinstance(x, dict):
        if not all(isinstance(k, str) for k in x): return '__NONCANON__'
        return '{{' + ', '.join(repr(k) + ': ' + canon(x[k]) for k in sorted(x)) + '}}'
    return '__NONCANON__'
def _load(p, n, side):
    # Module top-level code runs under the same budget; a load error is
    # re-raised unchanged so its message still reaches the verdict.
    m, e, out = _metered(load, p, n)
    if out:
        print('__BUDGET__\tload\t' + side); _sys.exit(0)
    if e is not None: raise e
    return m
cand = _load({cand:?}, "cand", 'candidate'); ref = _load({refr:?}, "ref", 'reference')
cf = getattr(cand, {fn:?}); rf = getattr(ref, {fn:?})
cases = [{cases}]
for i, args in enumerate(cases):
    _cv, _ce, _oc = _metered(cf, *args)
    _rv, _re, _or = _metered(rf, *args)
    if _oc or _or:
        _side = 'candidate and reference' if (_oc and _or) else ('candidate' if _oc else 'reference')
        print(f"__BUDGET__\t{{i}}\t{{_side}}"); _sys.exit(0)
    try: cv = canon(_cv) if _ce is None else None; ce = None if _ce is None else type(_ce).__name__
    except Exception as e: cv = None; ce = type(e).__name__
    try: rv = canon(_rv) if _re is None else None; re_ = None if _re is None else type(_re).__name__
    except Exception as e: rv = None; re_ = type(e).__name__
    if (cv is not None and '__NONCANON__' in cv) or (rv is not None and '__NONCANON__' in rv):
        print('__REFUSED__\treturns a value that is not JSON-structural (no stable cross-host form)'); _sys.exit(0)
    if ce is not None or re_ is not None:
        ok = (ce == re_)
        # Show each side's outcome: its exception name if it raised, else its
        # actual value. (Previously printed ce/re_ for both, so the non-raising
        # side reported None instead of its real value.)
        dc = ce if ce is not None else cv
        dr = re_ if re_ is not None else rv
        print(f"{{i}}\t{{'EQ' if ok else 'NE'}}\t{{dc}}\t{{dr}}")
    else:
        print(f"{{i}}\t{{'EQ' if cv == rv else 'NE'}}\t{{cv}}\t{{rv}}")
"#,
        admis = admis,
        cand = cand,
        refr = refr,
        fn = spec.func,
        cases = cases_py.join(", "),
        budget = spec.budget,
    )
}

/// Run the oracle. `candidate_path`/`reference_path` each define `spec.func`.
/// Float admissibility (AD-2) is enforced inside the driver as an AST allowlist
/// at source level; an inadmissible function surfaces as a `Refused` verdict.
pub fn review(candidate_path: &Path, reference_path: &Path, spec: &ReviewSpec) -> ReviewReceipt {
    let cand_src = std::fs::read(candidate_path).unwrap_or_default();
    let ref_src = std::fs::read(reference_path).unwrap_or_default();
    let cand_text = String::from_utf8_lossy(&cand_src);
    let ref_text = String::from_utf8_lossy(&ref_src);
    let cases = gen_cases(spec, &[&cand_text, &ref_text]);

    let verdict = run_python(candidate_path, reference_path, spec, &cases)
        .unwrap_or_else(|reason| ReviewVerdict::Error { reason });

    ReviewReceipt {
        candidate_sha256: sha256(&cand_src),
        reference_sha256: sha256(&ref_src),
        spec_sha256: sha256(&spec.canonical()),
        verdict,
        checker_version: env!("CARGO_PKG_VERSION").into(),
        budget: spec.budget,
    }
}

// ---------- PR review loop: base vs head ----------

/// One function to check in a PR: its new (head) source vs its base source.
pub struct PrCheck {
    pub name: String,
    pub head_path: std::path::PathBuf,
    pub base_path: std::path::PathBuf,
    pub spec: ReviewSpec,
}

pub struct ReviewItem {
    pub name: String,
    pub receipt: ReviewReceipt,
}

/// Run every check (head = candidate, base = reference). Returns the items
/// plus a CI gate exit code: 0 all equivalent, 1 any divergence, 2 only
/// errors. As a PR gate, nonzero blocks the merge.
pub fn run_pr(checks: &[PrCheck]) -> (Vec<ReviewItem>, i32) {
    let mut items = Vec::new();
    let mut any_cex = false;
    let mut any_err = false;
    for c in checks {
        // head is the candidate (the PR's new code), base is the reference.
        let receipt = review(&c.head_path, &c.base_path, &c.spec);
        match &receipt.verdict {
            ReviewVerdict::Counterexample { .. } => any_cex = true,
            ReviewVerdict::Error { .. } | ReviewVerdict::Refused { .. } => any_err = true,
            ReviewVerdict::Equivalent { .. } => {}
        }
        items.push(ReviewItem { name: c.name.clone(), receipt });
    }
    let code = if any_cex {
        1
    } else if any_err {
        2
    } else {
        0
    };
    (items, code)
}

// ---------- auto-detect changed functions (close the manifest bypass) ----------

/// What auto-detection decided about one changed function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedStatus {
    /// Present in base and head, body changed, every parameter has a supported
    /// hint. The signature is a simple positional match. Ready to check.
    Checkable { args: Vec<ArgType> },
    /// Changed but cannot be checked by equivalence. Surfaced, never dropped, so
    /// a green result never hides it.
    NotChecked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub name: String,
    pub status: DetectedStatus,
}

const DETECTOR_PY: &str = r#"import ast, sys
def ann(a):
    if a is None: return None
    if isinstance(a, ast.Name):
        return {'int':'int','str':'str','float':'float','dict':'dict','Dict':'dict'}.get(a.id)
    if isinstance(a, ast.Subscript):
        base = a.value.id if isinstance(a.value, ast.Name) else None
        if base in ('list','List') and isinstance(a.slice, ast.Name) and a.slice.id == 'int':
            return 'list[int]'
        if base in ('dict','Dict'):
            return 'dict'
    return None
def simple(fn):
    a = fn.args
    return not (a.vararg or a.kwarg or a.kwonlyargs or a.posonlyargs or a.defaults or a.kw_defaults)
def collect(body, prefix, in_class, in_func, out):
    # Every function at every nesting level, keyed by qualified name. Nothing
    # (async, methods, nested) is invisible to detection.
    for n in body:
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            q = prefix + n.name
            if isinstance(n, ast.AsyncFunctionDef): kind = 'async'
            elif in_class: kind = 'method'
            elif in_func: kind = 'nested'
            else: kind = 'func'
            out[q] = (n, kind)
            collect(n.body, q + '.', False, True, out)
        elif isinstance(n, ast.ClassDef):
            collect(n.body, prefix + n.name + '.', True, in_func, out)
def fns(path):
    try: tree = ast.parse(open(path, encoding='utf-8').read())
    except Exception: return None
    out = {}
    collect(tree.body, '', False, False, out)
    return out
b = fns(sys.argv[1])
h = fns(sys.argv[2])
if h is None:
    print('ERROR\t\thead source could not be parsed'); sys.exit(0)
if b is None:
    for q in sorted(h):
        print('NOTCHECKED\t' + q + '\tbase version could not be parsed; cannot diff')
    sys.exit(0)
for name in sorted(set(b) | set(h)):
    if name in h and name not in b:
        print('NOTCHECKED\t' + name + '\tnew function, no base version to compare against'); continue
    if name in b and name not in h:
        print('NOTCHECKED\t' + name + '\tremoved in this change'); continue
    bnode, _bk = b[name]; hnode, hkind = h[name]
    if ast.dump(bnode) == ast.dump(hnode): continue
    if hkind == 'async':
        print('NOTCHECKED\t' + name + '\tasync function, not checked in this version'); continue
    if hkind == 'method':
        print('NOTCHECKED\t' + name + '\tmethod, not checked (module-level functions only)'); continue
    if hkind == 'nested':
        print('NOTCHECKED\t' + name + '\tnested function, not checked'); continue
    if len(bnode.args.args) != len(hnode.args.args) or not simple(hnode) or not simple(bnode):
        print('NOTCHECKED\t' + name + '\tsignature is not a simple positional match'); continue
    sig, ok = [], True
    for a in hnode.args.args:
        t = ann(a.annotation)
        if t is None: ok = False; break
        sig.append(t)
    if ok:
        print('CHECKABLE\t' + name + '\t' + ','.join(sig))
    else:
        print('NOTCHECKED\t' + name + '\tno supported type hint on every parameter')
"#;

/// Detect which module-level functions changed between `base_src` and
/// `head_src` and classify each (close the manifest bypass, issue #1). The
/// invariant: a changed function is never silently dropped. Unchanged functions
/// are simply absent.
pub fn detect_changed_functions(base_src: &str, head_src: &str) -> Result<Vec<Detected>, String> {
    let tag = hex(&sha256(format!("{base_src}\u{0}{head_src}").as_bytes()))[..16].to_string();
    let dir = std::env::temp_dir();
    let bpath = dir.join(format!("equiv_detb_{tag}.py"));
    let hpath = dir.join(format!("equiv_deth_{tag}.py"));
    let spath = dir.join(format!("equiv_dets_{tag}.py"));
    std::fs::write(&bpath, base_src).map_err(|e| e.to_string())?;
    std::fs::write(&hpath, head_src).map_err(|e| e.to_string())?;
    std::fs::write(&spath, DETECTOR_PY).map_err(|e| e.to_string())?;
    let out = Command::new("python3")
        .arg(&spath)
        .arg(&bpath)
        .arg(&hpath)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .map_err(|e| format!("python3 not runnable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("detector error").to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut detected = Vec::new();
    for line in stdout.lines() {
        let p: Vec<&str> = line.splitn(3, '\t').collect();
        if p.len() < 3 {
            continue;
        }
        let (kind, name, rest) = (p[0], p[1].to_string(), p[2]);
        match kind {
            "ERROR" => return Err(rest.to_string()),
            "CHECKABLE" => {
                let args: Vec<ArgType> = rest
                    .split(',')
                    .filter(|s| !s.trim().is_empty())
                    .filter_map(ArgType::parse)
                    .collect();
                detected.push(Detected { name, status: DetectedStatus::Checkable { args } });
            }
            "NOTCHECKED" => {
                detected.push(Detected { name, status: DetectedStatus::NotChecked { reason: rest.to_string() } });
            }
            _ => {}
        }
    }
    Ok(detected)
}

/// Render the PR review and, beneath it, the changed-but-not-checked list. This
/// is what makes a green result honest: anything auto-detection could not check
/// is shown by name, never silently absent.
pub fn render_markdown_with_unchecked(
    items: &[ReviewItem],
    signer: Option<&str>,
    unchecked: &[(String, String)],
) -> String {
    let mut s = render_markdown(items, signer);
    if !unchecked.is_empty() {
        s.push_str("\n## changed, not checked\n\n");
        s.push_str(
            "These functions changed but equiv did not check them. A green result \
             above does not cover them.\n\n| function | reason |\n|---|---|\n",
        );
        for (name, reason) in unchecked {
            s.push_str(&format!("| `{}` | {} |\n", name, reason.replace('`', "")));
        }
    }
    s
}

fn short(id: &[u8; 32]) -> String {
    hex(id)[..12].to_string()
}

/// Build the in-toto Statement (DR-3) for one reviewed function: the payload
/// the keyless Sigstore path (cosign) signs. `file` is the reviewed path;
/// the subject digest is the candidate (head) source hash from the receipt.
pub fn intoto_statement(file: &str, item: &ReviewItem) -> String {
    use equiv_core::attest::{statement, ReviewPredicate};
    let (verdict, detail) = match &item.receipt.verdict {
        ReviewVerdict::Equivalent { n } => {
            ("equivalent".to_string(), format!("agreed on {n} cases"))
        }
        ReviewVerdict::Counterexample { input, candidate, reference } => (
            "counterexample".to_string(),
            format!("diverges at {input}: this={candidate} base={reference}"),
        ),
        ReviewVerdict::Error { reason } => ("error".to_string(), reason.clone()),
        ReviewVerdict::Refused { reason } => ("refused".to_string(), reason.clone()),
    };
    let receipt_id = hex(&item.receipt.sha256());
    let predicate = ReviewPredicate {
        verdict: &verdict,
        detail: &detail,
        receipt_id_hex: &receipt_id,
        checker: "equiv-review",
        checker_version: env!("CARGO_PKG_VERSION"),
    };
    statement(
        &format!("{file}::{}", item.name),
        &hex(&item.receipt.candidate_sha256),
        &predicate.to_json(),
    )
}

/// Sticky-comment marker so re-runs update one comment.
pub const MARKER: &str = "<!-- equiv-review -->";

/// Render the PR review as a GitHub-flavoured Markdown comment. `signer`, if
/// present, is the hex ed25519 public key that signed the receipts, shown so
/// reviewers/auditors can confirm it is the org's key.
pub fn render_markdown(items: &[ReviewItem], signer: Option<&str>) -> String {
    let mut diverged = 0;
    let mut errored = 0;
    for it in items {
        match it.receipt.verdict {
            ReviewVerdict::Counterexample { .. } => diverged += 1,
            ReviewVerdict::Error { .. } | ReviewVerdict::Refused { .. } => errored += 1,
            ReviewVerdict::Equivalent { .. } => {}
        }
    }
    let total = items.len();
    let fns = |n: usize| if n == 1 { "function" } else { "functions" };
    let headline = if diverged > 0 {
        format!("{diverged} of {total} checked {} changed behaviour.", fns(total))
    } else if errored > 0 {
        format!("{errored} of {total} {} could not be checked.", fns(total))
    } else {
        let verb = if total == 1 { "function: behaviour" } else { "functions: behaviour" };
        format!("{total} checked {verb} preserved against base.")
    };

    let mut s = String::new();
    s.push_str(MARKER);
    s.push('\n');
    s.push_str("## equiv review\n\n");
    s.push_str(&headline);
    s.push_str("\n\n| function | result | detail |\n|---|---|---|\n");
    for it in items {
        let (v, detail) = match &it.receipt.verdict {
            ReviewVerdict::Equivalent { n } => (
                "equivalent".to_string(),
                format!("{n}/{n} generated inputs agree"),
            ),
            ReviewVerdict::Counterexample { input, candidate, reference } => (
                "DIVERGES".to_string(),
                format!("input `{input}`: this PR returns `{candidate}`, base returns `{reference}`"),
            ),
            ReviewVerdict::Error { reason } => {
                ("not checked".to_string(), format!("`{}`", reason.replace('`', "")))
            }
            ReviewVerdict::Refused { reason } => {
                ("refused".to_string(), reason.replace('`', ""))
            }
        };
        s.push_str(&format!("| `{}` | {} | {} |\n", it.name, v, detail));
    }

    let summary = if signer.is_some() {
        "Receipts (deterministic, re-runnable, signed)"
    } else {
        "Receipts (deterministic, re-runnable)"
    };
    s.push_str(&format!("\n<details><summary>{summary}</summary>\n\n"));
    if let Some(pk) = signer {
        s.push_str(&format!("Signed (ed25519) by `{pk}`.\n\n"));
    }
    for it in items {
        s.push_str(&format!(
            "- `{}`: receipt-id {}\n",
            it.name,
            short(&it.receipt.sha256())
        ));
    }
    s.push_str("\n</details>\n\n");
    s.push_str(
        "Scope: behavioural equivalence on generated inputs only. This does not check \
         intent, architecture, security. A passing result means behaviour was preserved \
         on the tested inputs, not that the change is correct.\n",
    );
    s
}

fn run_python(
    cand: &Path,
    refr: &Path,
    spec: &ReviewSpec,
    cases: &[Vec<PyVal>],
) -> Result<ReviewVerdict, String> {
    let src = driver_source(cand, refr, spec, cases);
    // Content-addressed driver path: unique per distinct review (no clobber
    // between concurrent runs), stable for the same review.
    let tag = hex(&sha256(src.as_bytes()))[..16].to_string();
    let driver = std::env::temp_dir().join(format!("equiv_review_{tag}.py"));
    let mut f = std::fs::File::create(&driver).map_err(|e| e.to_string())?;
    f.write_all(src.as_bytes()).map_err(|e| e.to_string())?;
    drop(f);

    let out = Command::new("python3")
        .arg(&driver)
        // Harden for determinism: stable hashing, no user site packages.
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("")
        .output()
        .map_err(|e| format!("python3 not runnable: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().last().unwrap_or("python error").to_string();
        return Ok(ReviewVerdict::Error { reason: last });
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The driver's AST allowlist refuses inadmissible float functions before any
    // case runs, signalled on its own line.
    for line in stdout.lines() {
        if let Some(reason) = line.strip_prefix("__REFUSED__\t") {
            return Ok(ReviewVerdict::Refused { reason: reason.to_string() });
        }
        if let Some(found) = line.strip_prefix("__NOBUDGET__\t") {
            return Ok(ReviewVerdict::Error {
                reason: format!(
                    "the step budget needs Python 3.12 or newer (sys.monitoring); found {found}"
                ),
            });
        }
    }
    let budget = spec.budget;
    for line in stdout.lines() {
        // Lines arrive in case order and the driver stops at the first budget
        // exhaustion, so a counterexample printed earlier still stands. The
        // reason states budget, case, input and side, never the number of
        // events consumed (that may differ between Python versions).
        if let Some(rest) = line.strip_prefix("__BUDGET__\t") {
            let (at, side) = rest.split_once('\t').unwrap_or((rest, "candidate"));
            let reason = if at == "load" {
                format!(
                    "loading the {side} module exceeded the step budget of {budget} Python \
                     line/jump events; a non-terminating or very long run is not judged"
                )
            } else {
                let idx: usize = at.parse().map_err(|_| "bad index".to_string())?;
                let input: Vec<String> = cases[idx].iter().map(|v| v.display()).collect();
                format!(
                    "the {side} exceeded the step budget of {budget} Python line/jump events \
                     at input ({}); a non-terminating or very long run is not judged",
                    input.join(", ")
                )
            };
            return Ok(ReviewVerdict::Error { reason });
        }
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() < 4 {
            continue;
        }
        if parts[1] == "NE" {
            let idx: usize = parts[0].parse().map_err(|_| "bad index".to_string())?;
            let input: Vec<String> = cases[idx].iter().map(|v| v.display()).collect();
            return Ok(ReviewVerdict::Counterexample {
                input: format!("({})", input.join(", ")),
                candidate: parts[2].to_string(),
                reference: parts[3].to_string(),
            });
        }
    }
    // n is the number of cases actually checked (boundary + literal + random,
    // deduplicated), not just the requested random count: an honest tally.
    Ok(ReviewVerdict::Equivalent { n: cases.len() as u32 })
}
