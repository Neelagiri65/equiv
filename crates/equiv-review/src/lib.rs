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
    /// Two values; both are fixed corners on every review.
    Bool,
    /// A list of strings: the string corners lifted into lists, plus the
    /// orderings that sorts and tie-breaks care about.
    ListStr,
}

impl ArgType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "int" => ArgType::Int,
            "str" => ArgType::Str,
            "list[int]" | "list" => ArgType::ListInt,
            "float" => ArgType::Float,
            "dict" | "dict[str,int]" => ArgType::Dict,
            "bool" => ArgType::Bool,
            "list[str]" | "List[str]" => ArgType::ListStr,
            _ => return None,
        })
    }

    /// Parse a signature token with an optional input domain (0.23.0):
    /// `int[lo..hi]` (inclusive), `str[ascii]`, `str[1]` (exactly one code
    /// point), and the same inside lists: `list[int[1..12]]`, `list[str[1]]`.
    /// A plain token parses as with `parse`, with `Domain::Any`.
    pub fn parse_domain(s: &str) -> Option<(Self, Domain)> {
        let s = s.trim();
        if let Some(t) = Self::parse(s) {
            return Some((t, Domain::Any));
        }
        let (outer, inner) = match s.strip_prefix("list[").and_then(|r| r.strip_suffix(']')) {
            Some(inner) => (true, inner.trim()),
            None => (false, s),
        };
        let (base, dom) = inner.strip_suffix(']').and_then(|r| r.split_once('['))?;
        let dom = dom.trim();
        let (ty, d) = match base.trim() {
            "int" => {
                let (lo, hi) = dom.split_once("..")?;
                let lo: i64 = lo.trim().parse().ok()?;
                let hi: i64 = hi.trim().parse().ok()?;
                if lo > hi {
                    return None;
                }
                (if outer { ArgType::ListInt } else { ArgType::Int }, Domain::IntRange(lo, hi))
            }
            "str" => {
                let d = match dom {
                    "ascii" => Domain::Ascii,
                    "1" => Domain::OneChar,
                    _ => {
                        let src = dom.strip_prefix("in ")?.trim();
                        let ok = !src.is_empty()
                            && !src.ends_with('.')
                            && src.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                        if !ok {
                            return None;
                        }
                        Domain::OneOf { source: src.to_string(), values: Vec::new() }
                    }
                };
                (if outer { ArgType::ListStr } else { ArgType::Str }, d)
            }
            _ => return None,
        };
        Some((ty, d))
    }
}

/// The declared domains, readable: "arg 1 int[1..9999], arg 3 str[1]".
/// None when every argument is unconstrained.
pub fn domains_note(spec: &ReviewSpec) -> Option<String> {
    domains_note_tried(spec, &ValueSetCoverage::default())
}

/// What the generated inputs covered of the value-set arguments.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ValueSetCoverage {
    /// Per argument: distinct list values among the inputs (None for an
    /// argument that is not a value set).
    tried: Vec<Option<usize>>,
    /// With two or more `str[in ...]` arguments: (distinct combinations of
    /// their values among the inputs, all possible combinations; None when
    /// that number does not fit 128 bits).
    combos: Option<(u128, Option<u128>)>,
}

/// Count, from the case list itself, what was tried of every value set: the
/// figures the wording reports are measured on the inputs that ran, never
/// derived from how they were meant to be generated.
fn value_set_coverage(spec: &ReviewSpec, cases: &[Vec<PyVal>]) -> ValueSetCoverage {
    let tried: Vec<Option<usize>> = (0..spec.args.len())
        .map(|i| match dom(spec, i) {
            Domain::OneOf { .. } => {
                let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
                for c in cases {
                    match c.get(i) {
                        Some(PyVal::Str(s)) => {
                            seen.insert(s.as_str());
                        }
                        Some(PyVal::ListStr(xs)) => {
                            for s in xs {
                                seen.insert(s.as_str());
                            }
                        }
                        _ => {}
                    }
                }
                Some(seen.len())
            }
            _ => None,
        })
        .collect();
    let scalar_sets: Vec<usize> = (0..spec.args.len())
        .filter(|&i| spec.args[i] == ArgType::Str && matches!(dom(spec, i), Domain::OneOf { .. }))
        .collect();
    let combos = (scalar_sets.len() >= 2).then(|| {
        let mut seen: std::collections::BTreeSet<Vec<&str>> = std::collections::BTreeSet::new();
        for c in cases {
            let t: Option<Vec<&str>> = scalar_sets
                .iter()
                .map(|&i| match c.get(i) {
                    Some(PyVal::Str(s)) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            if let Some(t) = t {
                seen.insert(t);
            }
        }
        let total = scalar_sets.iter().try_fold(1u128, |acc, &i| match dom(spec, i) {
            Domain::OneOf { values, .. } => acc.checked_mul(values.len() as u128),
            _ => Some(acc),
        });
        (seen.len() as u128, total)
    });
    ValueSetCoverage { tried, combos }
}

/// As `domains_note`, plus what was tried of each value set when that is less
/// than all of it: "only X tried" for an argument whose list was not judged
/// completely, what a list argument's lists covered, and how many of the
/// combinations across several value-set arguments were tried.
fn domains_note_tried(spec: &ReviewSpec, cov: &ValueSetCoverage) -> Option<String> {
    if !has_domains(spec) {
        return None;
    }
    let tried = &cov.tried;
    let parts: Vec<String> = spec
        .args
        .iter()
        .enumerate()
        .filter(|(i, _)| *dom(spec, *i) != Domain::Any)
        .map(|(i, t)| {
            let (d, count) = match dom(spec, i) {
                Domain::IntRange(lo, hi) => (format!("[{lo}..{hi}]"), String::new()),
                Domain::Ascii => ("[ascii]".into(), String::new()),
                Domain::OneChar => ("[1]".into(), String::new()),
                Domain::OneOf { source, values } => {
                    let n = values.len();
                    let t = tried.get(i).copied().flatten();
                    let cov = match t {
                        Some(t) if t < n => format!("; only {t} tried"),
                        // A list argument: every value was in some list, but
                        // lists of several values are drawn, not enumerated.
                        Some(_) if *spec_ty(spec, i) == ArgType::ListStr => "; each tried in at least one list".to_string(),
                        _ => String::new(),
                    };
                    let within = if t.is_some() && *spec_ty(spec, i) == ArgType::ListStr {
                        "; combinations of values within a list sampled"
                    } else {
                        ""
                    };
                    (format!("[in {source}]"), format!(" ({n} values{cov}{within})"))
                }
                Domain::Any => (String::new(), String::new()),
            };
            let label = match t {
                ArgType::Int => format!("int{d}"),
                ArgType::Str => format!("str{d}"),
                ArgType::ListInt => format!("list[int{d}]"),
                ArgType::ListStr => format!("list[str{d}]"),
                other => format!("{other:?}{d}"),
            };
            format!("arg {} {label}{count}", i + 1)
        })
        .collect();
    let mut note = parts.join(", ");
    if let Some((got, total)) = cov.combos {
        if total.is_none_or(|t| got < t) {
            let of = match total {
                Some(t) => t.to_string(),
                None => "more than 10^38".to_string(),
            };
            note.push_str(&format!("; combinations across value-set arguments sampled: only {got} of {of} tried"));
        }
    }
    Some(note)
}

fn spec_ty(spec: &ReviewSpec, i: usize) -> &ArgType {
    &spec.args[i]
}

/// An opt-in input domain for one argument (0.23.0). For a list type it
/// constrains the elements. Generation, corners, source literals and harvested
/// inputs all stay inside it; a review under a domain is bound into the
/// receipt, so it can never be mistaken for a review of the whole type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Domain {
    Any,
    /// Inclusive integer range.
    IntRange(i64, i64),
    /// Strings of ASCII characters only.
    Ascii,
    /// Exactly one code point (Python `len(s) == 1`).
    OneChar,
    /// One of a fixed list of strings from the project's own code (0.24.0):
    /// `str[in mod.NAME]`, `str[in .constant.NAME]` (relative to the
    /// candidate's package) or `str[in NAME]` (the candidate module). Parsed
    /// with `values` empty; resolved by the review before any input is
    /// generated, and the resolved values are bound into the spec hash.
    OneOf { source: String, values: Vec<String> },
}

/// Default step budget per call: how many Python line and jump events one call
/// of the candidate or the reference may raise before the review stops and
/// reports could-not-check. Counted events, never wall-clock time, so the same
/// budget gives the same verdict on every host (gate P3).
pub const DEFAULT_BUDGET: u64 = 4_000_000;

/// Random inputs drawn per review when none is given (both `review` and
/// `review-pr`). Raised from 200/300 in 0.14.0: a shipped bug in a real port
/// (jellyfish #247) was reached on 0 of 5 seeds at 200 and 5 of 5 at 1,500+.
/// Cost on the divergence corpus is about 5x (median 0.03 -> 0.25 s/function).
pub const DEFAULT_N: u32 = 2_000;

/// Default wall-clock safety limit for one function's check, in seconds
/// (backlog 0.3b). Work inside a single C call (`sum(range(10**18))`) raises
/// no Python events, so the step budget cannot stop it; past this limit the
/// Python process group is killed. Wall-clock time must never decide a verdict
/// (gate P3), so a safety stop is not a receipt: it is a `SafetyStop`, which
/// has no bytes to hash, sign or attest. The limit is not in the spec hash or
/// the receipt.
pub const DEFAULT_SAFETY_SECONDS: u64 = 120;

/// A check that was killed by the wall-clock safety limit. Deliberately not a
/// `ReviewVerdict` and not inside a `ReviewReceipt`: whether it happens depends
/// on host speed, so it is reported as could-not-check and never receipted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyStop {
    pub func: String,
    pub limit_secs: u64,
}

impl SafetyStop {
    pub fn reason(&self) -> String {
        format!(
            "could not check: exceeded {} s safety limit (not part of the deterministic budget); \
             the check of `{}` was stopped and no receipt was issued",
            self.limit_secs, self.func
        )
    }
    /// Could not check: never green.
    pub fn exit_code(&self) -> i32 {
        2
    }
}

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
    /// Parity mode (0.23.0): when both sides raise, count it as agreement
    /// whatever the exception class. Twins (a Rust/C/Cython port and its
    /// Python reference) routinely reject bad input with different classes;
    /// without this, those rejections bury the real differences. A side that
    /// raises against a side that returns is still a difference. Bound into
    /// the spec hash only when on, so strict receipts keep their ids.
    pub errors_agree: bool,
    /// Per-argument input domains, by position (0.23.0). Empty, short, or
    /// all-`Any` means unconstrained, and then the spec hash is unchanged.
    pub domains: Vec<Domain>,
    /// Same-host float tier (0.26.0; reopens DECISIONS #17 as an opt-in):
    /// lift the cross-host float refusals (libm functions, calls equiv cannot
    /// analyse) and judge on this machine only. Bound into the spec hash only
    /// when on; the receipt tier is then `tested-same-host`.
    pub same_host: bool,
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
                ArgType::Bool => 6,
                ArgType::ListStr => 7,
            });
        }
        v.extend_from_slice(&self.n.to_le_bytes());
        v.extend_from_slice(&self.seed.to_le_bytes());
        v.extend_from_slice(&self.budget.to_le_bytes());
        if self.errors_agree {
            v.extend_from_slice(b"\0errors-agree");
        }
        if self.same_host {
            v.extend_from_slice(b"\0same-host");
        }
        // Input domains (0.23.0): only when some argument has one, so every
        // unconstrained receipt keeps its id. One tag per argument position,
        // int bounds at fixed width.
        if has_domains(self) {
            v.extend_from_slice(b"\0domains");
            for i in 0..self.args.len() {
                match dom(self, i) {
                    Domain::Any => v.push(0),
                    Domain::IntRange(lo, hi) => {
                        v.push(1);
                        v.extend_from_slice(&lo.to_le_bytes());
                        v.extend_from_slice(&hi.to_le_bytes());
                    }
                    Domain::Ascii => v.push(2),
                    Domain::OneChar => v.push(3),
                    // The source name and every resolved value, so a pass is
                    // bound to the exact list it was drawn from.
                    Domain::OneOf { source, values } => {
                        v.push(4);
                        v.extend_from_slice(source.as_bytes());
                        v.push(0);
                        v.extend_from_slice(&(values.len() as u32).to_le_bytes());
                        for s in values {
                            v.extend_from_slice(s.as_bytes());
                            v.push(0);
                        }
                    }
                }
            }
        }
        v
    }
}

pub mod harvest;
pub use harvest::Slots;

// ---------- deterministic input generation (Rust-side, seeded) ----------

struct Rng(u64);

/// The random stream for a seed. xorshift needs a non-zero state. Odd seeds
/// are used as they are (so seed 1 and every odd-seed receipt keep their
/// stream); even seeds were once made odd with `| 1`, which ran seed 2 as
/// seed 3, so they are mixed with splitmix64 into their own odd state.
fn seed_rng(seed: u64) -> Rng {
    if seed & 1 == 1 {
        return Rng(seed);
    }
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    Rng((z ^ (z >> 31)) | 1)
}
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
    Bool(bool),
    ListStr(Vec<String>),
    /// A str as UTF-16 code units, for the strings a Rust String cannot hold:
    /// lone surrogates. Only generated as str corners; handed to the
    /// evaluator as a Python literal with explicit escapes, like any str.
    StrUnits(Vec<u16>),
}

impl PyVal {
    /// A Python literal: what we hand the evaluator. Floats are reconstructed
    /// from exact bits via struct (no decimal parse means no rounding on the
    /// way in); the driver imports `struct`.
    fn to_py(&self) -> String {
        match self {
            PyVal::Int(n) => n.to_string(),
            PyVal::Str(s) => py_str_literal(s),
            PyVal::StrUnits(units) => py_units_literal(units),
            PyVal::ListInt(xs) => {
                let inner: Vec<String> = xs.iter().map(|n| n.to_string()).collect();
                format!("[{}]", inner.join(", "))
            }
            PyVal::Float(bits) => {
                format!("struct.unpack('>d', bytes.fromhex('{bits:016x}'))[0]")
            }
            PyVal::Bool(b) => if *b { "True".to_string() } else { "False".to_string() },
            PyVal::ListStr(xs) => {
                let inner: Vec<String> = xs.iter().map(|x| py_str_literal(x)).collect();
                format!("[{}]", inner.join(", "))
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
// The same literal for a sequence of UTF-16 code units: paired surrogates
// decode to their character, a lone surrogate is written as its \u escape
// (which Python accepts in a str literal). Equal to py_str_literal for any
// sequence that is valid UTF-16.
fn py_units_literal(units: &[u16]) -> String {
    let mut out = String::from("'");
    for r in char::decode_utf16(units.iter().copied()) {
        match r {
            Ok(ch) => {
                let lit = py_str_literal(&ch.to_string());
                out.push_str(&lit[1..lit.len() - 1]);
            }
            Err(e) => out.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    out.push('\'');
    out
}

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
        // Three shapes (0.17.0). Half the draws stay lowercase a-z, len <= 8,
        // as before. A quarter mix case, digits, punctuation, whitespace and
        // control characters from a small alphabet (small, so two strings that
        // differ only by case or one character turn up together). A quarter
        // are numeric text: optional leading whitespace (including \t \r \n
        // and U+00A0), optional sign, 1-4 digits, optional letter suffix.
        // Before 0.17 no random string held a capital, and digits or control
        // characters never sat in a multi-element list: the uutils sort
        // replay (2026-11-05) found nothing for exactly that reason.
        ArgType::Str => {
            let s: String = match rng.next() % 4 {
                0 | 1 => {
                    let len = (rng.next() % 9) as usize;
                    (0..len).map(|_| (b'a' + (rng.next() % 26) as u8) as char).collect()
                }
                2 => {
                    let len = (rng.next() % 9) as usize;
                    (0..len).map(|_| MIXED_CHARS[(rng.next() % MIXED_CHARS.len() as u64) as usize]).collect()
                }
                _ => {
                    let mut s = String::new();
                    if rng.next() % 3 == 0 {
                        s.push(NUM_PAD[(rng.next() % NUM_PAD.len() as u64) as usize]);
                    }
                    match rng.next() % 6 {
                        0 => s.push('-'),
                        1 => s.push('+'),
                        _ => {}
                    }
                    let digits = 1 + (rng.next() % 4) as usize;
                    for _ in 0..digits {
                        s.push((b'0' + (rng.next() % 10) as u8) as char);
                    }
                    if rng.next() % 3 == 0 {
                        for _ in 0..1 + rng.next() % 2 {
                            s.push(MIXED_CHARS[(rng.next() % 6) as usize]);
                        }
                    }
                    s
                }
            };
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
        ArgType::Bool => PyVal::Bool(rng.next() % 2 == 1),
        // Up to six strings, each drawn by the Str arm (len <= 8, three shapes).
        // In a quarter of non-empty lists, one more element is a near
        // duplicate of an earlier one: the same text, its upper- or lowercase
        // form, or the text with a leading space. Ties and case-only
        // differences are where sorting, grouping and de-duplication change;
        // independent draws almost never produce them (0.17.0).
        ArgType::ListStr => {
            let len = (rng.next() % 7) as usize;
            let mut v: Vec<String> = (0..len)
                .map(|_| match gen_arg(rng, ArgType::Str) {
                    PyVal::Str(s) => s,
                    _ => unreachable!(),
                })
                .collect();
            if len > 0 && rng.next() % 4 == 0 {
                let src = v[(rng.next() % len as u64) as usize].clone();
                let twin = match rng.next() % 4 {
                    0 => src.clone(),
                    1 => src.to_uppercase(),
                    2 => src.to_lowercase(),
                    _ => format!(" {src}"),
                };
                let at = (rng.next() % (len as u64 + 1)) as usize;
                v.insert(at, twin);
            }
            PyVal::ListStr(v)
        }
    }
}

// The mixed alphabet for random strings. The first six are letters (used
// alone for numeric-text suffixes): a, b, c and their capitals. Then digits,
// separators, whitespace, control characters and a few non-ASCII letters
// whose case mapping or width is unusual. Fixed and ordered: same draws on
// every host.
const MIXED_CHARS: &[char] = &[
    'a', 'b', 'c', 'A', 'B', 'C', '0', '1', '2', '9', ' ', '-', '_', '.', ',', '/', '\t', '\r', '\n', '\u{0b}',
    '\u{0c}', '\u{a0}', 'é', 'ß', 'İ',
];

// Leading padding for numeric text: where int(), float(), strip() and
// lstrip(' ') disagree.
const NUM_PAD: &[char] = &[' ', '\t', '\r', '\n', '\u{a0}', '0'];

// Magnitude envelope for synthesised integer inputs. A reference may be O(n),
// so inputs must not be astronomically large (would hang or OOM). The same
// bound the random generator respects, applied to boundary and literal inputs.
const INT_ENVELOPE: i64 = 1_000_000;

// Large-magnitude int corners (backlog 0.5b): the places where an int stops
// surviving a trip through a 32-bit field or a float64. 2^31 and 2^32 are
// the fixed-width wrap points; 2^49 + 1 is where log2(n).is_integer() starts
// lying; 2^53 is the last int float64 holds exactly, so n * 2.0, fabs and
// true division lose their last bit from 2^53 + 1; i64::MAX / i64::MIN are
// the widest this value type carries (2^63 + 1 is out of reach without a
// wider PyVal::Int, and is not part of this item). Each with its negative.
// They sit OUTSIDE the random envelope, so they are not in boundary_cases
// (which an O(n) reference must survive) but in their own block that runs
// after the random cases: see gen_cases.
const LARGE_INT_CORNERS: &[i64] = &[
    2_147_483_647,
    2_147_483_648,
    2_147_483_649,
    4_294_967_296,
    562_949_953_421_313,
    9_007_199_254_740_991,
    9_007_199_254_740_992,
    9_007_199_254_740_993,
    i64::MAX,
    -2_147_483_647,
    -2_147_483_648,
    -2_147_483_649,
    -4_294_967_296,
    -562_949_953_421_313,
    -9_007_199_254_740_991,
    -9_007_199_254_740_992,
    -9_007_199_254_740_993,
    -i64::MAX,
    i64::MIN,
];

// The value the other arguments hold while one argument is varied across its
// corners (one-at-a-time enumeration keeps the case count linear, not a
// cartesian explosion). These are the simplest values, and most of them are
// degenerate (0, "", [], {}, False), so a corner of one argument never meets
// an ordinary value of another here; the capped cross cases (see
// `cross_partners`) pair each argument's corners and literals with a few
// non-degenerate values of the others.
fn default_val(ty: ArgType) -> PyVal {
    match ty {
        ArgType::Int => PyVal::Int(0),
        ArgType::Str => PyVal::Str(String::new()),
        ArgType::ListInt => PyVal::ListInt(Vec::new()),
        ArgType::Float => PyVal::Float(0x3FF0_0000_0000_0000), // 1.0
        ArgType::Dict => PyVal::Dict(Vec::new()),
        ArgType::Bool => PyVal::Bool(false),
        ArgType::ListStr => PyVal::ListStr(Vec::new()),
    }
}

// String corners the random generator (len<=8, small alphabet) rarely or never reaches. Grouped by
// the semantics they stress; each is a well-known place where string
// refactors change behaviour. Fixed and ordered, so the case list is the same
// on every host. (A lone surrogate cannot be held in a Rust String; those
// corners are SURROGATE_CORNERS, carried as UTF-16 code units.)
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
    // numeric text: a leading sign (zfill vs rjust, isdecimal vs int()), a
    // decimal point and an exponent (float() accepts both), whitespace-padded
    // digits (int() and float() ignore the padding), and signed / decimal zero
    "-1", "+1", "1.5", "1e3", " 7 ", "-0", "+0", "0.0",
    // the rest of the int()/float() text grammar, where hand-written parsers
    // and an added strip() disagree with it: the four characters str.strip()
    // removes but int() and float() reject (U+001C-U+001F, the only such
    // characters), runs of signs (rejected), and PEP 515 underscores (one
    // between digits accepted; leading, trailing, doubled rejected)
    "\u{1c}7\u{1c}", "\u{1d}7\u{1d}", "\u{1e}7\u{1e}", "\u{1f}7\u{1f}",
    "--1", "+-1", "-+1", "++1",
    "1_000", "_1", "1_", "1__0", "1_0.5",
];

const SURROGATE_CORNERS: &[&[u16]] = &[&[0xD800], &[0xDFFF], &[0x78, 0xDC80]];

// Deterministic corner inputs the seeded random generator reaches rarely or
// never: the exact shapes where refactors break. All stay inside the O(n)-safe
// envelope so a linear reference cannot hang on them.
fn boundary_cases(ty: ArgType) -> Vec<PyVal> {
    match ty {
        // Off-by-one and sign corners (0, +/-1, +/-2) within the envelope.
        ArgType::Int => [0, 1, -1, 2, -2].into_iter().map(PyVal::Int).collect(),
        // Shapes the random generator (len<=8) rarely reaches: empty,
        // uppercase, digit, whitespace, and a string longer than 8.
        // (Lone surrogates are str corners too, but they run after the random
        // inputs; see the SURROGATE_CORNERS block after the random draw.)
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
            // exact halves: round() (half to even) and floor(x + 0.5) (half
            // up) differ only on ties, which random fractions rarely hit;
            // both parities, both signs
            0x3FE0_0000_0000_0000,    // 0.5
            0xBFE0_0000_0000_0000,    // -0.5
            0x3FF8_0000_0000_0000,    // 1.5
            0x4004_0000_0000_0000,    // 2.5
            0xC004_0000_0000_0000,    // -2.5
        ]
        .into_iter()
        .map(PyVal::Float)
        .collect(),
        ArgType::Dict => vec![
            PyVal::Dict(vec![]),
            PyVal::Dict(vec![("a".into(), 0)]),
            PyVal::Dict(vec![("a".into(), 1), ("b".into(), 2)]),
        ],
        // Both values, always.
        ArgType::Bool => vec![PyVal::Bool(false), PyVal::Bool(true)],
        // Structural corners (empty, one empty string, a tie, an ordered pair
        // and its reverse, a case pair, longer than the random cap) and every
        // string corner as a singleton list, so lower()/casefold()/strip()
        // style divergences are reachable inside a list too.
        ArgType::ListStr => {
            let s = |xs: &[&str]| PyVal::ListStr(xs.iter().map(|x| x.to_string()).collect());
            let mut v = vec![
                s(&[]),
                s(&[""]),
                s(&["a"]),
                s(&["a", "a"]),
                s(&["ab", "cd"]),
                s(&["a", "b"]),
                s(&["b", "a"]),
                s(&["A", "a"]),
                s(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]),
            ];
            v.extend(STR_CORNERS.iter().map(|c| s(&[c])));
            // Each corner again between two ordinary strings, out of order:
            // a singleton list cannot show an ordering, grouping or
            // neighbour difference (0.17.0).
            v.extend(STR_CORNERS.iter().filter(|c| !c.is_empty()).map(|c| s(&["b", c, "a"])));
            v
        }
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

// Float literals in the source: a decimal point (`1.5`, `.5`, `5.`), an
// exponent (`1e-9`, `1E9`, `2.5e3`) or both, with optional underscores between
// digits. The sign is not part of the token; both signs are offered (see
// `literal_inputs`). Rust and Python both round a decimal to the nearest
// float64, so the value is the one the function compares against, on every
// host. A token scan, like the int scan: a superset, which only adds inputs.
fn scan_float_literals(src: &str) -> Vec<f64> {
    let b = src.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    // Digits with single underscores between them; returns the new index.
    let digits = |from: usize, text: &mut String| -> usize {
        let mut k = from;
        while k < b.len() {
            if b[k].is_ascii_digit() {
                text.push(b[k] as char);
                k += 1;
            } else if b[k] == b'_' && k > from && k + 1 < b.len() && b[k + 1].is_ascii_digit() {
                k += 1;
            } else {
                break;
            }
        }
        k
    };
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            continue;
        }
        let lead_dot = c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit());
        if !(c.is_ascii_digit() || lead_dot) {
            i += 1;
            continue;
        }
        let mut text = String::new();
        let mut is_float = false;
        i = digits(i, &mut text);
        if i < b.len() && b[i] == b'.' {
            is_float = true;
            text.push('.');
            i = digits(i + 1, &mut text);
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut exp = String::from("e");
            let mut k = i + 1;
            if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                exp.push(b[k] as char);
                k += 1;
            }
            let before = exp.len();
            let end = digits(k, &mut exp);
            if exp.len() > before {
                is_float = true;
                text.push_str(&exp);
                i = end;
            }
        }
        // 0x10, 1.5j, 1e: not a float literal; skip the rest of the token.
        if i < b.len() && ident(b[i]) {
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            continue;
        }
        if is_float {
            if let Ok(f) = text.parse::<f64>() {
                if f.is_finite() {
                    out.push(f);
                }
            }
        }
    }
    out
}

// String literals in the source, decoded exactly as Python decodes them, so a
// branch on "\t", "\033" or "é" is tested with that exact text. Every escape
// of a str literal is resolved: \a \b \f \n \r \t \v, \\ and the quotes, a
// backslash-newline (nothing), octal (one to three digits), \xHH, \uXXXX and
// \UXXXXXXXX; any other backslash pair keeps its backslash, as Python does
// ("\d" is two characters). A literal that cannot be decoded exactly here is
// skipped, never guessed: \N{name} (no name table), a lone surrogate (a Rust
// String cannot hold one), a malformed \x, \u or \U. The whole prefix is read,
// in any case and order: r, rb, br, rf, fr, rt, tr are raw (backslashes
// kept); a bytes literal (b, rb, br) is not a str and is not offered. Before
// 0.28.0 octal escapes became NUL plus digits, \a \b \f \v lost their
// meaning, and only a prefix ending in r was raw. Still a token scan, not a
// parser (an f-string is offered with its braces, a triple-quoted literal in
// pieces): a superset, which only ever adds inputs.
fn scan_str_literals(src: &str) -> Vec<String> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let q = cs[i];
        if q == '\'' || q == '"' {
            let mut p = i;
            while p > 0 && (cs[p - 1].is_alphanumeric() || cs[p - 1] == '_') {
                p -= 1;
            }
            let prefix: String = cs[p..i].iter().collect::<String>().to_ascii_lowercase();
            // (raw, bytes); a run of letters that is not a prefix is a
            // keyword or name before the quote (`x or"y"`): no prefix.
            let (raw, bytes) = match prefix.as_str() {
                "r" | "fr" | "rf" | "tr" | "rt" => (true, false),
                "b" => (false, true),
                "br" | "rb" => (true, true),
                _ => (false, false),
            };
            let mut j = i + 1;
            let mut s = String::new();
            let mut closed = false;
            let mut exact = !bytes;
            while j < cs.len() {
                let c = cs[j];
                if c == '\\' && j + 1 < cs.len() {
                    let e = cs[j + 1];
                    if raw {
                        s.push(c);
                        s.push(e);
                        j += 2;
                        continue;
                    }
                    // `len` hex digits from `from`, as a char; None when a
                    // digit is missing or the value is not a char (a
                    // surrogate, or past U+10FFFF).
                    let hex_at = |from: usize, len: usize| -> Option<char> {
                        let t = cs.get(from..from + len)?;
                        if !t.iter().all(|c| c.is_ascii_hexdigit()) {
                            return None;
                        }
                        char::from_u32(u32::from_str_radix(&t.iter().collect::<String>(), 16).ok()?)
                    };
                    let simple = |ch: char| (Some(Some(ch)), 2);
                    // (Some(Some(ch)): one character; Some(None): nothing;
                    // None: cannot decode exactly), characters consumed.
                    let (ch, used): (Option<Option<char>>, usize) = match e {
                        '\\' | '\'' | '"' => simple(e),
                        'a' => simple('\u{07}'),
                        'b' => simple('\u{08}'),
                        'f' => simple('\u{0c}'),
                        'n' => simple('\n'),
                        'r' => simple('\r'),
                        't' => simple('\t'),
                        'v' => simple('\u{0b}'),
                        '\n' => (Some(None), 2), // line continuation
                        '0'..='7' => {
                            let mut v = 0u32;
                            let mut k = 0;
                            while k < 3 && cs.get(j + 1 + k).is_some_and(|d| ('0'..='7').contains(d)) {
                                v = v * 8 + cs[j + 1 + k].to_digit(8).unwrap_or(0);
                                k += 1;
                            }
                            (char::from_u32(v).map(Some), 1 + k)
                        }
                        'x' => (hex_at(j + 2, 2).map(Some), 4),
                        'u' => (hex_at(j + 2, 4).map(Some), 6),
                        'U' => (hex_at(j + 2, 8).map(Some), 10),
                        'N' => (None, 2),
                        other => {
                            s.push('\\');
                            simple(other)
                        }
                    };
                    match ch {
                        Some(Some(ch)) => s.push(ch),
                        Some(None) => {}
                        None => exact = false,
                    }
                    // A malformed escape is not skipped past its backslash
                    // pair, so the closing quote is still found.
                    j += if ch.is_some() { used } else { 2 };
                    continue;
                }
                if c == q {
                    closed = true;
                    j += 1;
                    break;
                }
                if c == '\n' {
                    break; // not a one-line literal (or a triple-quoted one)
                }
                s.push(c);
                j += 1;
            }
            if closed && exact && s.chars().count() <= 64 {
                out.push(s);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

// The ints a source's int literals aim at: for each literal L, in source
// order, L-1, L, L+1 and then the same around -L. The scan reads a number
// without its sign (`n == -70000` holds the token 70000, and `-` may as well
// be a subtraction), so both signs are always offered; the argument's domain
// drops the ones outside it.
fn int_literal_targets(src: &str) -> Vec<i64> {
    let mut out = Vec::new();
    for n in scan_int_literals(src) {
        for base in [Some(n), n.checked_neg()].into_iter().flatten() {
            for d in [-1i64, 0, 1] {
                out.extend(base.checked_add(d));
            }
        }
    }
    out
}

// The floats a source's numeric literals aim at: every float literal and every
// int literal float64 holds exactly (an int threshold on a float argument,
// `x >= 100`), each with both signs and, for each of those, the next float
// below and above it: `>=` against `>` differs at the literal itself, and a
// tolerance rewritten by one unit in the last place differs next to it.
fn float_literal_targets(src: &str) -> Vec<u64> {
    const EXACT: u64 = 1 << 53;
    let mut out = Vec::new();
    let ints = scan_int_literals(src).into_iter().filter(|n| n.unsigned_abs() <= EXACT).map(|n| n as f64);
    for f in scan_float_literals(src).into_iter().chain(ints) {
        for g in [f, -f] {
            for h in [g, g.next_down(), g.next_up()] {
                if h.is_finite() {
                    out.push(h.to_bits());
                }
            }
        }
    }
    out
}

// Inputs derived from the source's own literals, per argument type. Ints and
// floats: see `int_literal_targets` and `float_literal_targets`. Strings are
// added by value. Lists and dicts get their literal-aimed inputs from
// `element_inputs`, where the pool of element values is built.
fn literal_inputs(src: &str, ty: ArgType) -> Vec<PyVal> {
    match ty {
        ArgType::Int => int_literal_targets(src).into_iter().map(PyVal::Int).collect(),
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
        ArgType::Float => float_literal_targets(src).into_iter().map(PyVal::Float).collect(),
        // A dict keyed by each string literal in the source, with falsy and
        // truthy values: `d.get("port") or 8080` only diverges when the key
        // is present and its value is falsy.
        ArgType::Dict => scan_str_literals(src)
            .into_iter()
            .filter(|k| !k.is_empty())
            .flat_map(|k| [PyVal::Dict(vec![(k.clone(), 0)]), PyVal::Dict(vec![(k, 1)])])
            .collect(),
        // Element values aimed at the source's literals: `element_inputs`.
        ArgType::ListInt => Vec::new(),
        ArgType::Bool => Vec::new(),
        // Each string literal as a singleton list.
        ArgType::ListStr => scan_str_literals(src).into_iter().map(|k| PyVal::ListStr(vec![k])).collect(),
    }
}

// Longest string synthesised from a length literal: covers common limits
// (255, 1024, 4096) without building huge inputs.
const STR_LEN_CAP: i64 = 4096;

// ---------- element values and cross cases (0.28.0) ----------
//
// Before 0.28.0 a list element or dict value never left -20..20 and was never
// aimed at a source literal (`[x for x in xs if x >= 100]` against `> 100`
// agreed on every input), and a corner of one argument only ever met the
// degenerate defaults of the others. Everything here ADDS inputs after the
// ones generated before: the earlier inputs, their order and the main random
// stream are unchanged, so nothing that was reached is lost.

/// At most this many literal-aimed values per element pool (eight int
/// literals' worth: each gives six values), in source order.
const ELEM_POOL_CAP: usize = 48;

/// Extra seeded cases with wider element values: one for every this many
/// random cases asked for (250 at the default 2,000).
const WIDE_ELEMENT_SHARE: u32 = 8;

/// Upper bound on the cross cases of one review, over all pairs of arguments.
const CROSS_CAP: usize = 480;

/// The values the elements of one list or dict argument are aimed at: ints
/// around the source's int literals (both signs, each with its neighbours)
/// and the source's string literals, inside the argument's domain, in source
/// order, without repeats. Empty for scalar arguments and for a value-set
/// domain (there the list itself is the pool).
#[derive(Default)]
struct ElemPool {
    ints: Vec<i64>,
    strs: Vec<String>,
}

fn elem_pool(sources: &[&str], ty: ArgType, d: &Domain) -> ElemPool {
    let mut pool = ElemPool::default();
    if matches!(d, Domain::OneOf { .. }) {
        return pool;
    }
    for src in sources {
        if matches!(ty, ArgType::ListInt | ArgType::Dict) {
            for v in int_literal_targets(src) {
                if pool.ints.len() < ELEM_POOL_CAP && !pool.ints.contains(&v) && in_domain(&PyVal::ListInt(vec![v]), d) {
                    pool.ints.push(v);
                }
            }
        }
        if matches!(ty, ArgType::ListStr | ArgType::Dict) {
            for v in scan_str_literals(src) {
                if pool.strs.len() < ELEM_POOL_CAP
                    && !v.is_empty()
                    && !pool.strs.contains(&v)
                    && in_domain(&PyVal::ListStr(vec![v.clone()]), d)
                {
                    pool.strs.push(v);
                }
            }
        }
    }
    pool
}

// Fixed inputs that put each pool value into a list or dict: for a list of
// ints, the value alone and next to the following pool value (so `[99, 100]`,
// `[100, 101]` sit on both sides of a threshold); for a dict, the value under
// the key "a" and under the first two string literals (`d.get("port") >= 8080`
// needs the key and the value together); for a list of strings, each literal
// between two ordinary strings, out of order (the singleton is a literal input
// already).
fn element_inputs(ty: ArgType, pool: &ElemPool) -> Vec<PyVal> {
    let mut out = Vec::new();
    match ty {
        ArgType::ListInt => {
            for (k, &v) in pool.ints.iter().enumerate() {
                out.push(PyVal::ListInt(vec![v]));
                if pool.ints.len() > 1 {
                    out.push(PyVal::ListInt(vec![v, pool.ints[(k + 1) % pool.ints.len()]]));
                }
            }
        }
        ArgType::Dict => {
            for &v in &pool.ints {
                out.push(PyVal::Dict(vec![("a".into(), v)]));
            }
            for key in pool.strs.iter().take(2) {
                for &v in &pool.ints {
                    out.push(PyVal::Dict(vec![(key.clone(), v)]));
                }
            }
        }
        ArgType::ListStr => {
            for lit in &pool.strs {
                out.push(PyVal::ListStr(vec!["b".into(), lit.clone(), "a".into()]));
            }
        }
        _ => {}
    }
    out
}

// One element value for the wider seeded lists: half the time a pool value
// (when there is one), otherwise an int from three ranges, the old -20..20
// among them so ties still turn up, or, under a domain, a draw inside it.
fn wide_int(rng: &mut Rng, d: &Domain, pool: &ElemPool) -> i64 {
    if !pool.ints.is_empty() && rng.next() % 2 == 0 {
        return pool.ints[(rng.next() % pool.ints.len() as u64) as usize];
    }
    match d {
        &Domain::IntRange(lo, hi) => int_in(rng, lo, hi),
        _ => match rng.next() % 3 {
            0 => rng.range(-20, 20),
            1 => rng.range(-1000, 1000),
            _ => rng.range(-INT_ENVELOPE, INT_ENVELOPE),
        },
    }
}

// A list or dict whose elements leave -20..20 and visit the source's
// literals; None for an argument this does not apply to (a scalar, or a list
// of strings with no literal to aim at: its random elements are already
// drawn from the whole string generator).
fn gen_wide(rng: &mut Rng, ty: ArgType, d: &Domain, pool: &ElemPool) -> Option<PyVal> {
    match ty {
        ArgType::ListInt => {
            let len = (rng.next() % 7) as usize;
            Some(PyVal::ListInt((0..len).map(|_| wide_int(rng, d, pool)).collect()))
        }
        ArgType::Dict => {
            let len = (rng.next() % 5) as usize;
            let mut pairs: Vec<(String, i64)> = Vec::new();
            for _ in 0..len {
                let k = if !pool.strs.is_empty() && rng.next() % 3 == 0 {
                    pool.strs[(rng.next() % pool.strs.len() as u64) as usize].clone()
                } else {
                    ((b'a' + (rng.next() % 8) as u8) as char).to_string()
                };
                let v = wide_int(rng, d, pool);
                if !pairs.iter().any(|(have, _)| *have == k) {
                    pairs.push((k, v));
                }
            }
            Some(PyVal::Dict(pairs))
        }
        ArgType::ListStr if !pool.strs.is_empty() => {
            let len = (rng.next() % 7) as usize;
            let mut v = Vec::with_capacity(len);
            for _ in 0..len {
                if rng.next() % 2 == 0 {
                    v.push(pool.strs[(rng.next() % pool.strs.len() as u64) as usize].clone());
                } else if let PyVal::Str(x) = gen_in(rng, ArgType::Str, d) {
                    v.push(x);
                }
            }
            Some(PyVal::ListStr(v))
        }
        _ => None,
    }
}

// The second random stream, for the wider element cases: its own state, so
// the main stream and every input drawn from it stay exactly as they were.
fn wide_rng(seed: u64) -> Rng {
    let mut z = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xD1B5_4A32_D192_ED03);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    Rng((z ^ (z >> 31)) | 1)
}

// One argument's corners, then its literal inputs, then its literal-aimed
// element inputs; the second value is how many of them are corners.
fn arg_values(spec: &ReviewSpec, i: usize, sources: &[&str], pool: &ElemPool) -> (Vec<PyVal>, usize) {
    let ty = spec.args[i];
    let mut vals = domain_corners(ty, dom(spec, i));
    let n_corner = vals.len();
    for s in sources {
        vals.extend(literal_inputs(s, ty));
    }
    vals.extend(element_inputs(ty, pool));
    (vals, n_corner)
}

// The few non-degenerate values an argument takes while ANOTHER argument is
// run across its corners and literals (the cross cases). The defaults are
// mostly degenerate (0, "", []): `len(s) >= n` against a UTF-16 length agrees
// at n = 0 for every string. Ints: 1, 2 and the first four int literals of
// the source up to STR_LEN_CAP (a partner int may multiply the size of a
// corner of the other argument, as in `s * n`, so it stays small); strings:
// "a", "ab" and the first four string literals; one or two fixed values for
// the other types. Inside the
// argument's domain; when none of these is, the first two of the argument's
// own values that differ from its default. Bools have their own fan-out.
fn cross_partners(spec: &ReviewSpec, j: usize, sources: &[&str], own: &[PyVal], look: &Lookup, base: &PyVal) -> Vec<PyVal> {
    let s = |x: &str| x.to_string();
    let mut out: Vec<PyVal> = match spec.args[j] {
        ArgType::Int => {
            let mut v = vec![PyVal::Int(1), PyVal::Int(2)];
            for n in sources.iter().flat_map(|src| scan_int_literals(src)) {
                if v.len() < 6 && n.abs() <= STR_LEN_CAP && !v.contains(&PyVal::Int(n)) {
                    v.push(PyVal::Int(n));
                }
            }
            v
        }
        ArgType::Str => {
            let mut v = vec![PyVal::Str(s("a")), PyVal::Str(s("ab"))];
            for lit in sources.iter().flat_map(|src| scan_str_literals(src)) {
                if v.len() < 6 && !lit.is_empty() && !v.contains(&PyVal::Str(lit.clone())) {
                    v.push(PyVal::Str(lit));
                }
            }
            v
        }
        ArgType::Float => vec![PyVal::Float(0.5f64.to_bits()), PyVal::Float((-1.5f64).to_bits())],
        ArgType::ListInt => vec![PyVal::ListInt(vec![1, 2, 3]), PyVal::ListInt(vec![2, 1])],
        ArgType::ListStr => vec![PyVal::ListStr(vec![s("a"), s("b")])],
        ArgType::Dict => vec![PyVal::Dict(vec![(s("a"), 1), (s("b"), 2)])],
        ArgType::Bool => return Vec::new(),
    };
    out.retain(|v| v != base && look.one(v, j));
    if out.is_empty() {
        out = own.iter().filter(|v| *v != base).take(2).cloned().collect();
    }
    out
}

// A list or dict holding an int outside the envelope an O(n) reference must
// survive. Such an input is costly in the same way a large corner is, so it
// is judged like one: a side that runs out of the step budget on it is "not
// judged at this input", never the end of the review.
fn costly_elements(c: &[PyVal]) -> bool {
    c.iter().any(|v| matches!(v, PyVal::ListInt(_) | PyVal::Dict(_)) && !harvest::inside_envelope(v))
}

// Deterministic, ordered case list: the all-default base first (simplest
// counterexample reported first), then one-at-a-time boundary + source-literal
// inputs, then the original seeded random cases. Deduplicated by Python repr so
// overlapping corners are not run twice. `sources` are the candidate and
// reference texts, scanned for branch literals. Same inputs => same cases on
// every host (AC-1); changing this generator is a versioned change (the
// receipt carries checker_version).
#[cfg(test)]
fn gen_cases(spec: &ReviewSpec, sources: &[&str]) -> Vec<Vec<PyVal>> {
    gen_cases_tail(spec, sources, &[]).0
}

// The case list plus the index where the large-magnitude int corners begin
// (== cases.len() when the signature has no int argument). Inside that
// index a budget exhaustion is an Error; from it on, a per-input "not
// judged" (see run_python).
#[cfg(test)]
fn gen_cases_tail(spec: &ReviewSpec, sources: &[&str], harvested: &[Slots]) -> (Vec<Vec<PyVal>>, usize, InputCounts) {
    let (cases, large_from, counts, _) = gen_cases_soft(spec, sources, harvested);
    (cases, large_from, counts)
}

// As gen_cases_tail, plus the indices of harvested inputs placed before the
// tail. A project's own test value may be expensive (a brute-force search
// over a long input): running out of budget on one is "not judged at this
// input", like a large corner, and never ends the review.
fn gen_cases_soft(
    spec: &ReviewSpec,
    sources: &[&str],
    harvested: &[Slots],
) -> (Vec<Vec<PyVal>>, usize, InputCounts, Vec<usize>) {
    let look = Lookup::new(spec);
    let pools: Vec<ElemPool> =
        spec.args.iter().enumerate().map(|(i, t)| elem_pool(sources, *t, dom(spec, i))).collect();
    let per_arg: Vec<(Vec<PyVal>, usize)> =
        (0..spec.args.len()).map(|i| arg_values(spec, i, sources, &pools[i])).collect();
    let mut soft: Vec<usize> = Vec::new();
    let mut cases: Vec<Vec<PyVal>> = Vec::new();
    let mut counts = InputCounts::default();
    let mut seen: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    fn key(c: &[PyVal]) -> Vec<String> {
        c.iter().map(|v| v.to_py()).collect()
    }

    if !spec.args.is_empty() {
        let base: Vec<PyVal> = base_vals(spec);
        if seen.insert(key(&base)) {
            cases.push(base.clone());
            counts.corner += 1;
        }
        // Harvested inputs (0.17.0) next: the values the project's own calls
        // and tests use, so a counterexample on a familiar input is reported
        // first. Complete calls as they are, then every harvested value one
        // argument at a time with the others at their defaults. Values
        // outside the envelope wait for the tail (see below).
        for h in harvested {
            if h.iter().all(Option::is_some) && h.iter().flatten().all(harvest::inside_envelope) {
                let c: Vec<PyVal> = h.iter().flatten().cloned().collect();
                if !look.case(&c) {
                    counts.out_of_domain += 1;
                } else if seen.insert(key(&c)) {
                    soft.push(cases.len());
                    cases.push(c);
                    counts.harvested += 1;
                }
            }
        }
        for h in harvested {
            for (i, v) in h.iter().enumerate() {
                if let Some(v) = v.as_ref().filter(|v| harvest::inside_envelope(v)) {
                    if !look.one(v, i) {
                        counts.out_of_domain += 1;
                        continue;
                    }
                    let mut c = base.clone();
                    c[i] = v.clone();
                    if seen.insert(key(&c)) {
                        soft.push(cases.len());
                        cases.push(c);
                        counts.harvested += 1;
                    }
                }
            }
        }
        for (i, (vals, n_corner)) in per_arg.iter().enumerate() {
            // Order is unchanged from before counting existed (corners, then
            // literals, per argument); only the attribution is recorded.
            let n_corner = *n_corner;
            for (k, v) in vals.iter().enumerate() {
                let mut c = base.clone();
                c[i] = v.clone();
                if !look.case(&c) {
                    counts.out_of_domain += 1;
                    continue;
                }
                if seen.insert(key(&c)) {
                    cases.push(c);
                    if k < n_corner {
                        counts.corner += 1;
                    } else {
                        counts.literal += 1;
                    }
                }
            }
        }
        // Bool fan-out (DECISIONS #21): the corner and literal inputs above
        // ran with every bool at its default (False), so a divergence behind
        // a flag was out of reach. Run them again once per bool argument with
        // that flag set to True, plus once with every flag True when there are
        // two or more. The cost stays linear in the number of bools; a
        // divergence that needs a strict subset of three or more flags is
        // still out of reach. Signatures without a bool add nothing.
        for fb in flag_bases(spec).into_iter().skip(1) {
            for (i, ty) in spec.args.iter().enumerate() {
                if *ty == ArgType::Bool {
                    continue;
                }
                let (vals, n_corner) = &per_arg[i];
                let n_corner = *n_corner;
                for (k, v) in vals.iter().enumerate() {
                    let mut c = fb.clone();
                    c[i] = v.clone();
                    if look.case(&c) && seen.insert(key(&c)) {
                        cases.push(c);
                        if k < n_corner {
                            counts.corner += 1;
                        } else {
                            counts.literal += 1;
                        }
                    }
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
                        if look.case(&c) && seen.insert(key(&c)) {
                            cases.push(c);
                            counts.corner += 1;
                        }
                    }
                }
            }
        }
    }

    if !spec.args.is_empty() {
        let base: Vec<PyVal> = base_vals(spec);
        // Joint numeric corners: a divergence that needs two arguments to be
        // related (cancelling magnitudes, equal values) is unreachable when
        // the others sit at their default. For every pair of same-type
        // numeric arguments a small fixed set is run: (x, -x), (-x, x) and
        // (x, x). Floats use 2^54 (the smallest power of two where x - 1 and
        // -x + 1 both round back to ±x, so `a + b + c` and `a + (b + c)`
        // separate) and the largest finite value, plus (+inf, -inf) both
        // ways. Ints stay inside INT_ENVELOPE. Fixed, so seed-free and the
        // same on every host; single-argument reviews are untouched.
        for ty in [ArgType::Int, ArgType::Float] {
            let pairs: Vec<(PyVal, PyVal)> = match ty {
                ArgType::Int => [1i64, 2, INT_ENVELOPE]
                    .into_iter()
                    .flat_map(|x| [(x, -x), (-x, x), (x, x)])
                    .map(|(a, b)| (PyVal::Int(a), PyVal::Int(b)))
                    .collect(),
                ArgType::Float => {
                    let two54 = 18014398509481984.0f64;
                    let mut v: Vec<(f64, f64)> = [two54, f64::MAX]
                        .into_iter()
                        .flat_map(|x| [(x, -x), (-x, x), (x, x)])
                        .collect();
                    v.push((f64::INFINITY, f64::NEG_INFINITY));
                    v.push((f64::NEG_INFINITY, f64::INFINITY));
                    v.into_iter().map(|(a, b)| (PyVal::Float(a.to_bits()), PyVal::Float(b.to_bits()))).collect()
                }
                _ => unreachable!(),
            };
            let idx: Vec<usize> = spec.args.iter().enumerate().filter(|(_, t)| **t == ty).map(|(i, _)| i).collect();
            for (x, &i) in idx.iter().enumerate() {
                for &j in &idx[x + 1..] {
                    for (l, r) in &pairs {
                        let mut c = base.clone();
                        c[i] = l.clone();
                        c[j] = r.clone();
                        if look.case(&c) && seen.insert(key(&c)) {
                            cases.push(c);
                            counts.corner += 1;
                        }
                    }
                }
            }
        }
    }

    // Cross cases: for every ordered pair of non-bool arguments (i, j), the
    // corners and literals of i again with j at each of its few partner
    // values (see `cross_partners`) and the rest at their defaults. Bounded:
    // CROSS_CAP over the whole review, shared equally between the pairs, each
    // pair taking its cases in order (every value of i with the first
    // partner, then with the second, ...) until its share is used. With two
    // arguments each direction has 240; with many arguments a pair's share
    // covers only the first corners. A combination that runs out of the step
    // budget cannot end the review: these inputs are judged like the large
    // corners.
    let crossable: Vec<usize> =
        spec.args.iter().enumerate().filter(|(_, t)| **t != ArgType::Bool).map(|(i, _)| i).collect();
    if crossable.len() >= 2 {
        let base: Vec<PyVal> = base_vals(spec);
        let partners: Vec<Vec<PyVal>> = (0..spec.args.len())
            .map(|j| cross_partners(spec, j, sources, &per_arg[j].0, &look, &base[j]))
            .collect();
        let share = (CROSS_CAP / (crossable.len() * (crossable.len() - 1))).max(1);
        let mut total = 0;
        for &i in &crossable {
            for &j in &crossable {
                if i == j {
                    continue;
                }
                let (vals, n_corner) = &per_arg[i];
                let mut used = 0;
                'pair: for w in &partners[j] {
                    for (k, v) in vals.iter().enumerate() {
                        if used >= share || total >= CROSS_CAP {
                            break 'pair;
                        }
                        // A literal outside the envelope stays with the
                        // defaults it always ran with: paired with a
                        // non-empty partner it could cost memory, which no
                        // step budget bounds (`s * n` at n = 2^32).
                        if !harvest::inside_envelope(v) {
                            continue;
                        }
                        let mut c = base.clone();
                        c[i] = v.clone();
                        c[j] = w.clone();
                        if look.case(&c) && seen.insert(key(&c)) {
                            soft.push(cases.len());
                            cases.push(c);
                            if k < *n_corner {
                                counts.corner += 1;
                            } else {
                                counts.literal += 1;
                            }
                            used += 1;
                            total += 1;
                        }
                    }
                }
            }
        }
    }

    let mut rng = seed_rng(spec.seed);
    for _ in 0..spec.n {
        let c: Vec<PyVal> = spec.args.iter().enumerate().map(|(i, t)| gen_in(&mut rng, *t, dom(spec, i))).collect();
        if seen.insert(key(&c)) {
            cases.push(c);
            counts.random += 1;
        }
    }

    // Wider element values: one more seeded case for every WIDE_ELEMENT_SHARE
    // random cases, from a stream of its own. Each list or dict argument
    // draws its elements from the source's literals and their neighbours and
    // from wider ranges (to the edge of the envelope); the other arguments
    // are drawn as in the cases above. Judged like the large corners: a list
    // of six elements near 1,000,000 can cost an O(sum) reference more than
    // the step budget.
    let wide: Vec<bool> = spec
        .args
        .iter()
        .enumerate()
        .map(|(i, t)| matches!(t, ArgType::ListInt | ArgType::Dict) || (*t == ArgType::ListStr && !pools[i].strs.is_empty()))
        .collect();
    if wide.iter().any(|w| *w) {
        let mut rng = wide_rng(spec.seed);
        for _ in 0..spec.n / WIDE_ELEMENT_SHARE {
            let c: Vec<PyVal> = spec
                .args
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let d = dom(spec, i);
                    gen_wide(&mut rng, *t, d, &pools[i]).unwrap_or_else(|| gen_in(&mut rng, *t, d))
                })
                .collect();
            if look.case(&c) && seen.insert(key(&c)) {
                soft.push(cases.len());
                cases.push(c);
                counts.random += 1;
            }
        }
    }

    // Lone surrogates (a Rust String cannot hold them): high, low, and a low
    // surrogate after text (the shape surrogateescape leaves for an
    // undecodable byte), one str argument at a time, on every flag base.
    // They run after the random inputs because a compiled twin that takes
    // &str (PyO3) rejects every one of them: the difference is real and still
    // counted, but an ordinary difference, when there is one, comes first and
    // is the headline. Path A step 2 finding (DECISIONS #34).
    if spec.args.iter().any(|t| *t == ArgType::Str) {
        for base in flag_bases(spec) {
            for (i, ty) in spec.args.iter().enumerate() {
                if *ty != ArgType::Str {
                    continue;
                }
                for u in SURROGATE_CORNERS {
                    let mut c = base.clone();
                    c[i] = PyVal::StrUnits(u.to_vec());
                    if look.case(&c) && seen.insert(key(&c)) {
                        cases.push(c);
                        counts.corner += 1;
                    }
                }
            }
        }
    }

    // Large-magnitude int corners, one argument at a time, LAST. A function
    // whose work grows with n exhausts the step budget on a 2^63 input; such
    // an input is recorded as not judged (counted in the receipt, printed
    // with the verdict) and the review goes on, so a linear reference is
    // still judged everywhere inside the envelope exactly as before. Running
    // these after every other case means a counterexample the small inputs
    // find still comes first. Fixed and seed-free, so the same on every host.
    let large_from = cases.len();
    if !spec.args.is_empty() {
        // Default base first (unchanged order), then once per bool set True.
        for base in flag_bases(spec) {
            for (i, ty) in spec.args.iter().enumerate() {
                if *ty != ArgType::Int {
                    continue;
                }
                for &n in LARGE_INT_CORNERS {
                    let mut c = base.clone();
                    c[i] = PyVal::Int(n);
                    if look.case(&c) && seen.insert(key(&c)) {
                        cases.push(c);
                        counts.corner += 1;
                    }
                }
            }
        }
        // The same corners as the only element of a list and as a dict value
        // (default base only): `sum(xs)` against a float sum separates at
        // 2^53 + 1 and nowhere inside the envelope.
        let base: Vec<PyVal> = base_vals(spec);
        for (i, ty) in spec.args.iter().enumerate() {
            for &n in LARGE_INT_CORNERS {
                let v = match ty {
                    ArgType::ListInt => PyVal::ListInt(vec![n]),
                    ArgType::Dict => PyVal::Dict(vec![("a".into(), n)]),
                    _ => break,
                };
                let mut c = base.clone();
                c[i] = v;
                if look.case(&c) && seen.insert(key(&c)) {
                    cases.push(c);
                    counts.corner += 1;
                }
            }
        }
        // Harvested values outside the envelope (a test's f(10**12)): here,
        // where a side running out of budget is "not judged at this input".
        for h in harvested {
            if h.iter().all(Option::is_some) && !h.iter().flatten().all(harvest::inside_envelope) {
                let c: Vec<PyVal> = h.iter().flatten().cloned().collect();
                if !look.case(&c) {
                    counts.out_of_domain += 1;
                } else if seen.insert(key(&c)) {
                    cases.push(c);
                    counts.harvested += 1;
                }
            }
            for (i, v) in h.iter().enumerate() {
                if let Some(v) = v.as_ref().filter(|v| !harvest::inside_envelope(v)) {
                    if !look.one(v, i) {
                        counts.out_of_domain += 1;
                        continue;
                    }
                    let mut c = base.clone();
                    c[i] = v.clone();
                    if seen.insert(key(&c)) {
                        cases.push(c);
                        counts.harvested += 1;
                    }
                }
            }
        }
    }
    // Value-set domains (0.25.0): every input is drawn from the project's own
    // list, like a harvested input, so one that runs past the step budget (a
    // parser on a long real file) is "not judged at this input", counted and
    // printed, and the review goes on; `--budget` judges it. Unconstrained and
    // other domains keep "exhaustion ends the review" as before.
    if spec.domains.iter().any(|d| matches!(d, Domain::OneOf { .. })) {
        soft = (0..large_from).collect();
    } else {
        // Literal-aimed lists and dicts may hold a value outside the envelope
        // (a mask such as 0xFFFFFFFF): see `costly_elements`.
        soft.extend((0..large_from).filter(|&k| costly_elements(&cases[k])));
        soft.sort_unstable();
        soft.dedup();
    }
    (cases, large_from, counts, soft)
}

// The default base (every argument at its default) followed by one base per
// bool argument with that flag set to True. Just the default base when the
// signature holds no bool, so other signatures generate exactly as before.
// Every argument sits at its in-domain default (see `default_in`).
fn flag_bases(spec: &ReviewSpec) -> Vec<Vec<PyVal>> {
    let base: Vec<PyVal> = base_vals(spec);
    let mut out = vec![base.clone()];
    for (i, ty) in spec.args.iter().enumerate() {
        if *ty == ArgType::Bool {
            let mut b = base.clone();
            b[i] = PyVal::Bool(true);
            out.push(b);
        }
    }
    // With two or more bools, one more base with every flag True: a
    // divergence gated on all flags together is reached for one extra pass,
    // whatever the number of bools (with two bools the bool space is then
    // covered completely). A single bool adds nothing: all-True is its
    // one-flag base. Path A step 3, corpus case 095.
    if spec.args.iter().filter(|t| **t == ArgType::Bool).count() >= 2 {
        out.push(
            base.iter().zip(&spec.args).map(|(v, t)| if *t == ArgType::Bool { PyVal::Bool(true) } else { v.clone() }).collect(),
        );
    }
    out
}


// ---------- input domains (0.23.0) ----------
//
// Opt-in per argument. With no domain (the default) every function below is
// the identity on the unconstrained path: same values, same order, no extra
// random draws, so unconstrained reviews generate byte-identical cases.

fn dom(spec: &ReviewSpec, i: usize) -> &Domain {
    static ANY: Domain = Domain::Any;
    spec.domains.get(i).unwrap_or(&ANY)
}

fn has_domains(spec: &ReviewSpec) -> bool {
    spec.domains.iter().any(|d| *d != Domain::Any)
}

/// Is `v` inside `d`? For a list, every element must be.
fn in_domain(v: &PyVal, d: &Domain) -> bool {
    let one = |s: &str| s.chars().count() == 1;
    match (d, v) {
        (Domain::Any, _) => true,
        (Domain::IntRange(lo, hi), PyVal::Int(x)) => lo <= x && x <= hi,
        (Domain::IntRange(lo, hi), PyVal::ListInt(xs)) => xs.iter().all(|x| lo <= x && x <= hi),
        (Domain::OneOf { values, .. }, PyVal::Str(s)) => values.iter().any(|x| x == s),
        (Domain::OneOf { .. }, PyVal::StrUnits(_)) => false,
        (Domain::OneOf { values, .. }, PyVal::ListStr(xs)) => xs.iter().all(|s| values.iter().any(|x| x == s)),
        (Domain::Ascii, PyVal::Str(s)) => s.is_ascii(),
        (Domain::Ascii, PyVal::StrUnits(u)) => u.iter().all(|c| *c < 0x80),
        (Domain::Ascii, PyVal::ListStr(xs)) => xs.iter().all(|s| s.is_ascii()),
        (Domain::OneChar, PyVal::Str(s)) => one(s),
        (Domain::OneChar, PyVal::StrUnits(u)) => u.len() == 1,
        (Domain::OneChar, PyVal::ListStr(xs)) => xs.iter().all(|s| one(s)),
        // parse_domain only pairs int ranges with int types and string
        // domains with string types; anything else is not constrained.
        _ => true,
    }
}

/// Domain membership for one review, with value-set lists in hash sets so a
/// 100,000-name list costs O(1) per check instead of a linear scan.
struct Lookup<'a> {
    spec: &'a ReviewSpec,
    sets: Vec<Option<std::collections::HashSet<&'a str>>>,
}

impl<'a> Lookup<'a> {
    fn new(spec: &'a ReviewSpec) -> Self {
        let sets = (0..spec.args.len())
            .map(|i| match dom(spec, i) {
                Domain::OneOf { values, .. } => Some(values.iter().map(|s| s.as_str()).collect()),
                _ => None,
            })
            .collect();
        Lookup { spec, sets }
    }
    fn one(&self, v: &PyVal, i: usize) -> bool {
        match self.sets.get(i).and_then(|s| s.as_ref()) {
            Some(set) => one_of(set, v),
            None => in_domain(v, dom(self.spec, i)),
        }
    }
    fn case(&self, c: &[PyVal]) -> bool {
        c.iter().enumerate().all(|(i, v)| self.one(v, i))
    }
}

fn one_of(set: &std::collections::HashSet<&str>, v: &PyVal) -> bool {
    match v {
        PyVal::Str(s) => set.contains(s.as_str()),
        PyVal::ListStr(xs) => xs.iter().all(|s| set.contains(s.as_str())),
        PyVal::StrUnits(_) => false,
        _ => true,
    }
}

/// The default value, moved inside the domain when it falls outside (an int
/// range that excludes 0 sits at the endpoint nearest 0; one character is
/// "a"), so one-argument-at-a-time cases are not all dropped for the others.
fn default_in(ty: ArgType, d: &Domain) -> PyVal {
    let v = default_val(ty);
    if in_domain(&v, d) {
        return v;
    }
    match (ty, d) {
        (ArgType::Int, Domain::IntRange(lo, hi)) => PyVal::Int(if *lo > 0 { *lo } else { *hi }),
        (ArgType::Str, Domain::OneChar) => PyVal::Str("a".into()),
        (ArgType::Str, Domain::OneOf { values, .. }) if !values.is_empty() => PyVal::Str(values[0].clone()),
        _ => v,
    }
}

fn base_vals(spec: &ReviewSpec) -> Vec<PyVal> {
    spec.args.iter().enumerate().map(|(i, t)| default_in(*t, dom(spec, i))).collect()
}

/// Corners for one argument: the unconstrained corners that fall inside the
/// domain, then corners of the domain itself (its endpoints and their
/// neighbours, 0 and ±1 when inside; for strings, in-domain characters that
/// separate implementations).
fn domain_corners(ty: ArgType, d: &Domain) -> Vec<PyVal> {
    if *d == Domain::Any {
        return boundary_cases(ty);
    }
    let mut v: Vec<PyVal> = boundary_cases(ty).into_iter().filter(|x| in_domain(x, d)).collect();
    let strs = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    const ASCII: &[&str] = &["a", "A", "0", " ", "\t", "\n", "\x7f", "Hello, World!", "  x  ", "-12"];
    const ONE: &[&str] = &["a", "A", "z", "0", " ", "\n", "é", "ß", "İ", "ǅ", "\u{3000}", "٣", "ﬁ", "😀"];
    match (ty, d) {
        (ArgType::Int, &Domain::IntRange(lo, hi)) => {
            let near = [Some(lo), lo.checked_add(1), hi.checked_sub(1), Some(hi), Some(0), Some(1), Some(-1)];
            v.extend(near.into_iter().flatten().filter(|x| lo <= *x && *x <= hi).map(PyVal::Int));
        }
        (ArgType::ListInt, &Domain::IntRange(lo, hi)) => {
            for xs in [vec![lo], vec![hi], vec![lo, hi], vec![hi, lo], vec![lo, lo]] {
                v.push(PyVal::ListInt(xs));
            }
        }
        (ArgType::Str, Domain::Ascii) => v.extend(ASCII.iter().map(|s| PyVal::Str(s.to_string()))),
        (ArgType::Str, Domain::OneChar) => v.extend(ONE.iter().map(|s| PyVal::Str(s.to_string()))),
        (ArgType::ListStr, Domain::Ascii) => {
            for xs in [&["a"][..], &["b", "a"], &["A", "a"], &["a", "a"], &[" a", "a"]] {
                v.push(PyVal::ListStr(strs(xs)));
            }
        }
        (ArgType::ListStr, Domain::OneChar) => {
            for xs in [&["a"][..], &["b", "a"], &["A", "a"], &["a", "a"], &["é"], &["ß", "S"], &["٣", "3"]] {
                v.push(PyVal::ListStr(strs(xs)));
            }
        }
        (ArgType::Str, Domain::OneOf { values, .. }) => {
            // A finite list is checked completely: every name is a corner, up
            // to VALUE_SET_ALL names. A longer list gets that many spread
            // across it (first and last included) plus random draws.
            v.extend(value_set_picks(values).map(|s| PyVal::Str(s.clone())));
        }
        (ArgType::ListStr, Domain::OneOf { values, .. }) if !values.is_empty() => {
            let a = values[0].clone();
            let b = values[values.len() - 1].clone();
            for xs in [vec![a.clone()], vec![a.clone(), b.clone()], vec![b.clone(), a.clone()], vec![a.clone(), a]] {
                v.push(PyVal::ListStr(xs));
            }
            // Every value on its own in a list, under the same cap as a
            // scalar value set, so "N values" never counts a value no list
            // held. Lists of several values are drawn, not enumerated, and
            // the receipt says so.
            v.extend(value_set_picks(values).map(|s| PyVal::ListStr(vec![s.clone()])));
        }
        _ => {}
    }
    match d {
        Domain::OneOf { values, .. } => {
            let set: std::collections::HashSet<&str> = values.iter().map(|s| s.as_str()).collect();
            v.retain(|x| one_of(&set, x));
        }
        _ => v.retain(|x| in_domain(x, d)),
    }
    v
}

/// Lists of up to this many strings (`str[in ...]`) have every value judged
/// for each argument, one argument at a time (0.27.0: raised from 4,096, which
/// sampled real lists such as SQL corpora silently). Combinations across two
/// or more such arguments are sampled, and the receipt says how many of them
/// were tried (0.28.0). A `list[str[in ...]]` argument holds every value in
/// at least one list; lists of several values are sampled, and the receipt
/// says that too. Above the cap, the receipt says how many values of the list
/// were tried.
const VALUE_SET_ALL: usize = 100_000;

/// The values of a value set that are tried one by one: all of them up to
/// VALUE_SET_ALL, otherwise that many spread evenly across the list (first
/// and last included).
fn value_set_picks(values: &[String]) -> Box<dyn Iterator<Item = &String> + '_> {
    let n = values.len();
    if n <= VALUE_SET_ALL {
        Box::new(values.iter())
    } else {
        Box::new((0..VALUE_SET_ALL).map(move |k| &values[spread_index(k, n, VALUE_SET_ALL)]))
    }
}

/// Position of pick `k` of `slots` spread evenly over `n` values. The product
/// is taken in 128 bits: in `usize` it wraps on a 32-bit target once
/// `k * (n - 1)` passes 2^32 (a list just over the cap), and a 32-bit host
/// would then sample different values than a 64-bit one.
fn spread_index(k: usize, n: usize, slots: usize) -> usize {
    (k as u128 * (n as u128 - 1) / (slots as u128 - 1)) as usize
}

/// A random int inside `lo..=hi`, drawn from the seeded stream.
fn int_in(rng: &mut Rng, lo: i64, hi: i64) -> i64 {
    let span = (hi as i128 - lo as i128 + 1) as u128;
    let pick = |rng: &mut Rng, w: u128| (rng.next() as u128 % w) as i128;
    // Log-scaled offset: magnitudes 1, 10, 1000, 10^6... all get drawn, so
    // a wide range still reaches ordinary values and not only its ends.
    let log = |rng: &mut Rng| {
        let bits = rng.next() % 32;
        pick(rng, (1u128 << bits).min(span))
    };
    let x = match rng.next() % 4 {
        0 => lo as i128 + log(rng),
        1 => hi as i128 - log(rng),
        // The unconstrained shape (0, ±12, ±1000, ±10^6) when it lands
        // inside, so small values near 0 are reached as in plain `int`.
        2 => match gen_arg(rng, ArgType::Int) {
            PyVal::Int(v) if lo <= v && v <= hi => v as i128,
            _ => lo as i128 + log(rng),
        },
        _ => lo as i128 + pick(rng, span),
    };
    x as i64
}

/// A random value inside the domain, drawn from the same seeded stream.
fn gen_in(rng: &mut Rng, ty: ArgType, d: &Domain) -> PyVal {
    fn str_in(rng: &mut Rng, d: &Domain) -> String {
        if let Domain::OneOf { values, .. } = d {
            if !values.is_empty() {
                return values[(rng.next() % values.len() as u64) as usize].clone();
            }
        }
        let s = match gen_arg(rng, ArgType::Str) {
            PyVal::Str(s) => s,
            _ => unreachable!(),
        };
        match d {
            Domain::Ascii => s.chars().map(|c| if c.is_ascii() { c } else { char::from((c as u32 % 0x80) as u8) }).collect(),
            Domain::OneChar => {
                let cs: Vec<char> = s.chars().collect();
                if cs.is_empty() {
                    char::from((0x20 + rng.next() % 0x5f) as u8).to_string()
                } else {
                    cs[(rng.next() % cs.len() as u64) as usize].to_string()
                }
            }
            _ => s,
        }
    }
    match (ty, d) {
        (_, Domain::Any) => gen_arg(rng, ty),
        (ArgType::Int, &Domain::IntRange(lo, hi)) => PyVal::Int(int_in(rng, lo, hi)),
        (ArgType::ListInt, &Domain::IntRange(lo, hi)) => {
            let len = (rng.next() % 7) as usize;
            PyVal::ListInt((0..len).map(|_| int_in(rng, lo, hi)).collect())
        }
        (ArgType::Str, _) => PyVal::Str(str_in(rng, d)),
        (ArgType::ListStr, _) => {
            let len = (rng.next() % 7) as usize;
            PyVal::ListStr((0..len).map(|_| str_in(rng, d)).collect())
        }
        _ => gen_arg(rng, ty),
    }
}

// Unit tests for the generation section (the scanners and the value-set
// picks are private). The other in-file tests are at the end of the file.
#[cfg(test)]
mod generation_tests {
    use super::*;

    // What the interpreter makes of each literal: the UTF-8 hex of a str,
    // "bytes" for a bytes literal, "unencodable" for a str holding a lone
    // surrogate; for a number, the float64 bits in hex.
    fn python(exprs: &[&str], numeric: bool) -> Vec<String> {
        let body = if numeric {
            "import struct, sys\nfor e in sys.argv[1:]:\n    print(struct.pack('>d', float(eval(e))).hex())\n"
        } else {
            "import sys, warnings\nwarnings.simplefilter('ignore')\nfor e in sys.argv[1:]:\n    v = eval(e)\n    if isinstance(v, bytes):\n        print('bytes')\n        continue\n    try:\n        print('s' + v.encode('utf-8').hex())\n    except UnicodeEncodeError:\n        print('unencodable')\n"
        };
        let out = evaluator_command().arg("-c").arg(body).args(exprs).output().expect("python3 runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect()
    }

    fn hex_of(s: &str) -> String {
        format!("s{}", hex(s.as_bytes()))
    }

    // Every literal is decoded exactly as Python decodes it, or not offered
    // at all; never offered as different text.
    #[test]
    fn string_literals_decode_as_python_does() {
        let lits: &[&str] = &[
            r#""plain""#, r#"'it\'s'"#, r#""say \"hi\"""#, r#""a\\b""#, r#""\a\b\f\n\r\t\v""#, r#""\0""#, r#""\033""#,
            r#""\7""#, r#""\18""#, r#""\101""#, r#""\1011""#, r#""\400""#, r#""\777""#, r#""\8\9""#, r#""\x41\xe9""#,
            r#""\u00e9\u20ac""#, r#""\U0001f600""#, r#""\d+\.\w""#, r#""\ ""#, r#""\N{BULLET}""#, r#""x\N{LATIN SMALL LETTER A}y""#,
            r#""\ud800""#, r#""\udfff!""#, r#"r"\d\n""#, r#"R"\t""#, r#"u"\t""#, r#"U"\x41""#, r#"f"\t""#, r#"F"\x41""#,
            r#"rf"\t""#, r#"fr"\t""#, r#"Rf"\t""#, r#"fR"\t""#, r#"RF"\033""#, r#"b"\t""#, r#"B"x""#, r#"rb"\d""#,
            r#"br"\d""#, r#"Rb"\d""#, r#"bR"\d""#, r#"BR"\d""#, r#""é ß 😀""#, r#"'\''"#, r#""\"""#, r#"r"\"""#,
        ];
        let want = python(lits, false);
        assert_eq!(want.len(), lits.len());
        for (lit, want) in lits.iter().zip(&want) {
            let got = scan_str_literals(&format!("x = {lit}\n"));
            if want == "bytes" || want == "unencodable" || lit.contains("\\N{") {
                assert!(got.is_empty(), "{lit} must not be offered, got {got:?}");
            } else {
                assert_eq!(got.len(), 1, "{lit}: {got:?}");
                assert_eq!(&hex_of(&got[0]), want, "{lit} decoded as {:?}", got[0]);
            }
        }
    }

    #[test]
    fn a_name_before_a_quote_is_not_a_prefix() {
        assert_eq!(scan_str_literals("y = x or\"\\t\"\n"), vec!["\t".to_string()]);
        assert_eq!(scan_str_literals("y = x if b else'\\033'\n"), vec!["\u{1b}".to_string()]);
        // A malformed escape drops only its own literal.
        assert_eq!(scan_str_literals("a = '\\xZZ'; b = 'kept'\n"), vec!["kept".to_string()]);
    }

    // Float literals are found in every form and have the bits Python gives
    // them.
    #[test]
    fn float_literals_are_scanned_in_every_form() {
        let lits: &[&str] =
            &["1.5", ".5", "5.", "1e-9", "1E9", "2.5e3", "1_000.25", "1e+2", "0.1", "1e300", "3.14159", "1_0e1_0", "0.30000000000000004"];
        let want = python(lits, true);
        for (lit, want) in lits.iter().zip(&want) {
            let got = scan_float_literals(&format!("if x >= {lit}:\n    pass\n"));
            assert_eq!(got.len(), 1, "{lit}: {got:?}");
            assert_eq!(&format!("{:016x}", got[0].to_bits()), want, "{lit}");
        }
        // Not floats: ints, hex, names with digits, attribute access, an
        // imaginary literal, an infinite value.
        for src in ["x = 100", "x = 0x1e5", "x1e5 = 2", "v2.e3", "x = 1.5j", "x = 1e999", "x = 0b101", "e5 = 1"] {
            assert!(scan_float_literals(src).is_empty(), "{src}: {:?}", scan_float_literals(src));
        }
        assert_eq!(scan_float_literals("a = [1.5, 2.5e1]; b = x-0.25"), vec![1.5, 25.0, 0.25]);
    }

    #[test]
    fn numeric_literals_are_offered_with_both_signs_and_neighbours() {
        let ints = int_literal_targets("if n == -70000:\n    pass\n");
        assert_eq!(ints, vec![69999, 70000, 70001, -70001, -70000, -69999]);
        let floats = float_literal_targets("return x >= 100 or x == 0.5\n");
        let has = |f: f64| floats.contains(&f.to_bits());
        for f in [0.5, -0.5, 100.0, -100.0] {
            assert!(has(f) && has(f.next_up()) && has(f.next_down()), "{f} and its neighbours: {floats:?}");
        }
        // An int float64 cannot hold exactly is not offered as a float.
        assert!(float_literal_targets("x = 9007199254740993\n").is_empty());
        // The widest literal has no +1 and its negative has no -1: nothing
        // wraps.
        let edge = int_literal_targets(&format!("x = {}\n", LITERAL_INT_CAP));
        assert!(edge.contains(&i64::MAX) && edge.contains(&-LITERAL_INT_CAP) && edge.contains(&(i64::MIN + 1)));
    }

    // The picks of a sampled value set, computed in 128 bits, are what exact
    // arithmetic gives, including where a 32-bit `usize` product would wrap.
    #[test]
    fn spread_index_does_not_depend_on_the_target_width() {
        let slots = VALUE_SET_ALL;
        for n in [slots + 1, 100_500, 250_000, 4_000_000, u32::MAX as usize] {
            assert_eq!(spread_index(0, n, slots), 0);
            assert_eq!(spread_index(slots - 1, n, slots), n - 1, "last pick of {n}");
            let mut prev = 0;
            for k in [1usize, 2, 42_949, 42_950, 50_000, 99_998] {
                let exact = (k as u128 * (n as u128 - 1) / (slots as u128 - 1)) as usize;
                let got = spread_index(k, n, slots);
                assert_eq!(got, exact, "k {k}, n {n}");
                assert!(got >= prev && got < n);
                prev = got;
                // Where the 32-bit product wraps, the wrapped pick differs:
                // this is the input that used to depend on the host.
                let wrapped = ((k as u32).wrapping_mul(n as u32 - 1) / (slots as u32 - 1)) as usize;
                if (k as u128 * (n as u128 - 1)) > u32::MAX as u128 {
                    assert_ne!(got, wrapped, "k {k}, n {n}: a 32-bit product would have agreed by accident");
                }
            }
        }
        let values: Vec<String> = (0..100_500).map(|i| i.to_string()).collect();
        let picks: Vec<&String> = value_set_picks(&values).collect();
        assert_eq!(picks.len(), VALUE_SET_ALL);
        assert_eq!((picks[0].as_str(), picks[VALUE_SET_ALL - 1].as_str()), ("0", "100499"));
        assert_eq!(picks[50_000].as_str(), "50250");
    }

    // The inputs generated before 0.28.0 are all still generated, in the
    // same order: the new ones only come after or between blocks.
    #[test]
    fn cross_cases_and_wide_elements_only_add_inputs() {
        let src = "def f(xs, n):\n    return [x for x in xs if x >= 100][:n]\n";
        let spec = ReviewSpec {
            func: "f".into(),
            args: vec![ArgType::ListInt, ArgType::Int],
            n: 400,
            seed: 1,
            budget: DEFAULT_BUDGET,
            errors_agree: false,
            domains: vec![],
            same_host: false,
        };
        let (cases, large_from, counts, soft) = gen_cases_soft(&spec, &[src], &[]);
        // The main random stream is untouched: its draws appear in order.
        let mut rng = seed_rng(1);
        let mut at = 0;
        for _ in 0..spec.n {
            let c: Vec<PyVal> = spec.args.iter().map(|t| gen_arg(&mut rng, *t)).collect();
            if let Some(pos) = cases[at..].iter().position(|x| *x == c) {
                at += pos + 1;
            } else {
                assert!(cases[..at].contains(&c), "random case {c:?} is missing or out of order");
            }
        }
        // Wide and cross cases sit before the tail and cannot end a review.
        let wide = cases[..large_from]
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(&c[0], PyVal::ListInt(xs) if xs.iter().any(|x| x.abs() > 20)))
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        assert!(wide.len() > 30, "only {} lists leave -20..20", wide.len());
        for k in &wide {
            let costly = costly_elements(&cases[*k]);
            assert!(soft.contains(k) || !costly, "case {k} {:?} could end the review", cases[*k]);
        }
        assert!(soft.windows(2).all(|w| w[0] < w[1]), "soft indices are sorted and distinct");
        assert_eq!(counts.corner + counts.literal + counts.random, cases.len() as u32);
        // Same spec, same list: no hidden state.
        assert_eq!(gen_cases_soft(&spec, &[src], &[]).0, cases);
    }
}

// ---------- verdict + receipt ----------

/// Evidence floor for an "equivalent". When at least THIN_K inputs were judged
/// but fewer than THIN_K of them returned a value on both sides (the rest raised
/// on both), the verdict rests on thin evidence and says so; when none returned
/// a value it is "could not check". A function with fewer than THIN_K possible
/// inputs (no arguments, a bool) is checked completely and exempt. Chosen from
/// the v0.18 replay, where value-returning counts are bimodal (47 equivalents
/// below 10, 3 between 10 and 100, 104 at 100 or more).
pub const THIN_K: u32 = 10;

/// The number of inputs that returned a value on both sides, when an
/// "equivalent" over `n` judged inputs rests on thin evidence; else None.
/// `exceptions` is the receipt's count of inputs that raised on both sides.
pub fn thin_evidence(n: u32, exceptions: u32) -> Option<u32> {
    let v = n.saturating_sub(exceptions);
    (n >= THIN_K && v > 0 && v < THIN_K).then_some(v)
}

/// The one phrase every renderer uses for a thin "equivalent".
pub fn thin_note(r: &ReviewReceipt) -> Option<String> {
    match r.verdict {
        ReviewVerdict::Equivalent { n, .. } => thin_evidence(n, r.exceptions).map(|v| {
            format!("thin evidence: only {v} of {n} inputs returned a value (the rest raised on both sides)")
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewVerdict {
    /// Candidate and reference agreed on all `n` judged cases. `not_judged`
    /// counts the large-magnitude int corners (outside the random envelope,
    /// run last) where a side exhausted the step budget: no verdict at that
    /// input, stated here and in the receipt bytes rather than silently
    /// dropped. Zero for any function that finishes at every corner.
    Equivalent { n: u32, not_judged: u32 },
    /// First input where they diverge (the review's payload), plus how many
    /// inputs were judged and on how many the two sides disagreed. `stopped`
    /// is empty unless the run ended early (a side exhausted the step budget
    /// after the first disagreement); it then names the budget and the input,
    /// and `judged` counts only the inputs run before the stop.
    Counterexample {
        input: String,
        candidate: String,
        reference: String,
        judged: u32,
        disagreeing: u32,
        stopped: String,
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
            ReviewVerdict::Equivalent { n, not_judged } => {
                Value::Array(vec![Value::Uint(0), Value::Uint(*n as u64), Value::Uint(*not_judged as u64)])
            }
            ReviewVerdict::Counterexample { input, candidate, reference, judged, disagreeing, stopped } => {
                Value::Array(vec![
                    Value::Uint(1),
                    Value::Text(input.clone()),
                    Value::Text(candidate.clone()),
                    Value::Text(reference.clone()),
                    Value::Uint(*judged as u64),
                    Value::Uint(*disagreeing as u64),
                    Value::Text(stopped.clone()),
                ])
            }
            ReviewVerdict::Error { reason } => {
                Value::Array(vec![Value::Uint(2), Value::Text(reason.clone())])
            }
            ReviewVerdict::Refused { reason } => {
                Value::Array(vec![Value::Uint(3), Value::Text(reason.clone())])
            }
        }
    }
}

/// One input on which the two sides disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disagreement {
    pub input: String,
    pub candidate: String,
    pub reference: String,
}

/// The line that follows a counterexample wherever it is shown: how many of
/// the judged inputs disagreed, and where the run stopped if it stopped early.
pub fn disagreements_note(v: &ReviewVerdict) -> String {
    match v {
        ReviewVerdict::Counterexample { judged, disagreeing, stopped, .. } => {
            let mut s = format!("disagreements: {disagreeing} of {judged} judged inputs");
            if !stopped.is_empty() {
                s.push_str(&format!(" (stopped: {stopped})"));
            }
            s
        }
        _ => String::new(),
    }
}

/// The clause appended to an equivalent verdict when large-magnitude corners
/// went unjudged; empty when none did. One wording everywhere it is printed.
pub fn not_judged_note(k: u32) -> String {
    if k == 0 {
        String::new()
    } else {
        // Large-magnitude corners, or the project's own (harvested) values,
        // on which a side ran out of the step budget.
        format!("; {k} large or costly input{} not judged (step budget)", if k == 1 { "" } else { "s" })
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
    /// The evaluator's Python `major.minor`, as reported by the interpreter
    /// that ran the cases. Line/jump event counts differ between minors, so
    /// the claim is "reproducible on the same evaluator version". Empty only
    /// when no evaluator ran at all (python3 not runnable).
    pub python: String,
    /// The seed the random cases were drawn with (also inside `spec_sha256`,
    /// here readable).
    pub seed: u64,
    /// How many inputs came from each source; see `InputCounts`.
    pub inputs: InputCounts,
    /// The declared input domains, readable (e.g. "arg 1 int[1..9999]");
    /// empty without domains. NOT in the receipt bytes: the domains
    /// themselves are bound through `spec_sha256`.
    pub domains: String,
    /// Judged inputs on which at least one side raised (and both agreed).
    pub exceptions: u32,
    /// The claim tier. This checker only ever produces `tested`: bounded
    /// testing on generated inputs, never a proof.
    pub tier: String,
    /// Every input on which the sides disagreed, in case order (the first is
    /// the verdict's headline). NOT in the receipt bytes: its length is
    /// (verdict `disagreeing`), and each entry is re-derived by re-running the
    /// pinned candidate, reference and spec. Empty unless a counterexample.
    pub disagreements: Vec<Disagreement>,
}

/// Inputs by source. `corner` is every fixed input (type corners, paired
/// string corners, joint numeric corners and the large-magnitude corners),
/// `literal` the inputs read from the sources' own literals, `random` the
/// seeded draws. Cases are deduplicated; a value two sources both produce is
/// credited to the first source that produced it, in the order the case list
/// is built. corner + literal + random = judged + not judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputCounts {
    pub corner: u32,
    pub literal: u32,
    pub random: u32,
    /// Inputs harvested from the project's own calls and tests (0.17.0).
    pub harvested: u32,
    /// Source literals and harvested values skipped because they fall
    /// outside a declared input domain (0.23.0). Zero without domains.
    pub out_of_domain: u32,
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
            (7, Value::Text(self.python.clone())),
            (8, Value::Uint(self.seed)),
            (
                9,
                Value::Array(vec![
                    Value::Uint(self.inputs.corner as u64),
                    Value::Uint(self.inputs.literal as u64),
                    Value::Uint(self.inputs.random as u64),
                    Value::Uint(self.inputs.harvested as u64),
                ]
                .into_iter()
                // Only under input domains, so other receipts keep their bytes.
                .chain((self.inputs.out_of_domain > 0).then(|| Value::Uint(self.inputs.out_of_domain as u64)))
                .collect()),
            ),
            (10, Value::Uint(self.exceptions as u64)),
            (11, Value::Text(self.tier.clone())),
        ];
        cbor::to_bytes(&Value::Map(entries))
    }
    pub fn sha256(&self) -> [u8; 32] {
        sha256(&self.to_bytes())
    }
}

// ---------- the oracle ----------

/// The evaluator for ONE side (`is_cand`: the candidate, else the reference).
/// Each side runs in its own interpreter, so neither can see or alter the
/// other's arguments, return values or loaded modules. The driver loads only
/// its own side's file and reports one outcome line per input, then an end
/// line; the comparison is done by the caller, on the reported bytes.
fn driver_source(is_cand: bool, cand: &Path, refr: &Path, spec: &ReviewSpec, cases: &[Vec<PyVal>], large_from: usize, soft: &[usize]) -> String {
    let cases_py: Vec<String> = cases
        .iter()
        .map(|c| {
            let args: Vec<String> = c.iter().map(|v| v.to_py()).collect();
            // `()` for a function that takes no arguments: `(,)` is not
            // Python, and every such review used to end in a SyntaxError.
            if args.is_empty() {
                "()".to_string()
            } else {
                format!("({},)", args.join(", "))
            }
        })
        .collect();
    // The evaluator decides nothing that reaches the receipt: it emits a
    // per-case status line; Rust interprets and builds the verdict. For float
    // reviews an AST allowlist (spec-type-admissibility.md AD-2) runs first, at
    // source level, before any module loads: a function is admitted only if it
    // stays inside the IEEE-754 correctly rounded closure (+ - * / and
    // math.sqrt and the exact math functions such as floor, comparisons, literals,
    // a small set of exact builtins). Anything
    // else (** , //, %, dynamic dispatch, unknown calls, foreign imports) is
    // refused by name. This is the allowlist the spec mandates, not a denylist.
    // Always on, every signature: a function body that reaches a maths-library
    // result is refused by name. libm's last bit (log, exp, sin, pow with a
    // float, ...) differs between hosts, so a verdict resting on it would not
    // be byte-identical everywhere. The exact set is what IEEE-754 or integer
    // arithmetic pins on every host. Walks the target function only and
    // follows `import math as m` / `from math import x as y`; a libm call
    // hidden in a helper or `(n / 2) ** 2` without a float literal is not
    // seen (DECISIONS #17).
    let libm = if spec.same_host { String::new() } else { format!(
        r#"import ast as _ast, sys as _sys
_EXACT = {{'sqrt', 'fabs', 'floor', 'ceil', 'trunc', 'copysign', 'fmod', 'modf', 'frexp', 'ldexp', 'fsum',
          'isqrt', 'gcd', 'lcm', 'factorial', 'comb', 'perm', 'isfinite', 'isnan', 'isinf'}}
_LIBM = ', a maths-library result whose last bit is not reproducible across hosts'
def _libm(_path):
    try: _t = _ast.parse(open(_path, encoding='utf-8').read())
    except Exception: return None
    _mods = {{}}
    _names = {{}}
    for _n in _ast.walk(_t):
        if isinstance(_n, _ast.Import):
            for _a in _n.names:
                if _a.name in ('math', 'cmath'): _mods[_a.asname or _a.name] = _a.name
        elif isinstance(_n, _ast.ImportFrom) and _n.module in ('math', 'cmath'):
            for _a in _n.names: _names[_a.asname or _a.name] = (_n.module, _a.name)
    _target = None
    for _n in _ast.walk(_t):
        if isinstance(_n, _ast.FunctionDef) and _n.name == {fn:?}: _target = _n; break
    if _target is None: return None
    for _node in _ast.walk(_target):
        if isinstance(_node, _ast.Call):
            _f = _node.func
            if isinstance(_f, _ast.Attribute) and isinstance(_f.value, _ast.Name) and _f.value.id in _mods:
                _m = _mods[_f.value.id]
                if _m == 'cmath' or _f.attr not in _EXACT: return 'reaches ' + _m + '.' + _f.attr + _LIBM
            if isinstance(_f, _ast.Name) and _f.id in _names:
                _m, _o = _names[_f.id]
                if _m == 'cmath' or _o not in _EXACT: return 'reaches ' + _m + '.' + _o + _LIBM
        if isinstance(_node, _ast.BinOp) and isinstance(_node.op, _ast.Pow):
            for _s in (_node.left, _node.right):
                if isinstance(_s, _ast.Constant) and isinstance(_s.value, float):
                    return 'uses ** (Pow) with a float operand: pow is' + _LIBM[1:]
    return None
for _p in ({cand:?}, {refr:?}):
    _w = _libm(_p)
    if _w:
        _emit('__REFUSED__\t' + _w); _stop()
"#,
        fn = spec.func,
        cand = cand,
        refr = refr,
    ) };
    // Same-host tier: both cross-host float refusals (libm above, the closure
    // check here) are lifted; nothing else is.
    let admis = if !spec.same_host && spec.args.iter().any(|a| *a == ArgType::Float) {
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
    _okmath = {{'sqrt', 'floor', 'ceil', 'trunc', 'fabs', 'copysign', 'fmod', 'modf', 'frexp', 'ldexp',
               'isfinite', 'isnan', 'isinf'}}
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
                if not (isinstance(_f.value, _ast.Name) and _f.value.id == 'math' and _f.attr in _okmath):
                    _nm = (_f.value.id + '.' + _f.attr) if isinstance(_f.value, _ast.Name) else _f.attr
                    return 'calls ' + _nm + ' outside the admissible set'
            else:
                return 'uses a dynamic call that cannot be analysed'
        if isinstance(_node, _ast.Attribute):
            if not (isinstance(_node.value, _ast.Name) and _node.value.id == 'math' and _node.attr in _okmath):
                return 'uses attribute ' + _node.attr + ' that cannot be analysed'
    return None
for _p in ({cand:?}, {refr:?}):
    _w = _bad(_p)
    if _w:
        _emit('__REFUSED__\t' + _w); _stop()
"#,
            fn = spec.func,
            cand = cand,
            refr = refr,
        )
    } else {
        String::new()
    };
    let (own, name, side) = if is_cand { (cand, "cand", "candidate") } else { (refr, "ref", "reference") };
    format!(
        r#"import importlib.util as u
import struct
import sys as _sys
import os as _os
# Result lines leave on a private copy of the original stdout. File descriptor
# 1 and sys.stdout are pointed at the null device first, so nothing the
# reviewed code prints can be read as a result line. Values cross as hex.
_out = _os.fdopen(_os.dup(1), 'w', encoding='utf-8', errors='backslashreplace', newline='\n')
_null = open(_os.devnull, 'w')
_os.dup2(_null.fileno(), 1)
_sys.stdout = _null
def _emit(s): _out.write(s + '\n')
def _stop():
    _out.flush(); _os._exit(0)
def _hex(s): return s.encode('utf-8', 'backslashreplace').hex()
_emit('__PY__\t' + str(_sys.version_info[0]) + '.' + str(_sys.version_info[1]))
{libm}{admis}# Deterministic step budget (backlog 0.3, gate P3). Every LINE and JUMP event
# raised by code outside this driver file counts one step; past the budget the
# callback raises _Budget, and keeps raising on every later event, so user code
# that swallows it (even a bare except) cannot loop on. No clock is read.
if _sys.version_info < (3, 12):
    _emit('__NOBUDGET__\t' + _sys.version.split()[0]); _stop()
_M = _sys.monitoring
_TOOL = 4
_EV = _M.events.LINE | _M.events.JUMP
_BUDGET = {budget}
# Cases from index _LARGE on are the large-magnitude int corners (outside
# the envelope). Exhausting the budget there is 'not judged at this input',
# reported and counted; before it, exhaustion ends this side's run.
_LARGE = {large_from}
# Harvested inputs before the tail: also 'not judged' on budget exhaustion.
_SOFT = {{{soft}}}
_left = [0]
# Counting is switched by a flag, with the events set once: set_events itself
# costs time in proportion to the live objects, so toggling it around every
# call made large reviews quadratic (0.27.0). The same events are counted
# inside each call as before; this file's own lines are disabled outright.
_on = [False]
class _Budget(BaseException): pass
def _tick(code, *_a):
    if code.co_filename == __file__: return _M.DISABLE
    if not _on[0]: return
    _left[0] -= 1
    if _left[0] < 0: raise _Budget()
_M.use_tool_id(_TOOL, 'equiv-budget')
_M.register_callback(_TOOL, _M.events.LINE, _tick)
_M.register_callback(_TOOL, _M.events.JUMP, _tick)
_M.set_events(_TOOL, _EV)
def _metered(fn, *args):
    # (value, exception or None, budget exhausted?)
    _left[0] = _BUDGET; _on[0] = True
    try:
        v = fn(*args); e = None
    except BaseException as x:
        if _left[0] < 0 or isinstance(x, _Budget): return None, None, True
        # A Rust panic through PyO3 (pyo3_runtime.PanicException) derives from
        # BaseException. It is a deterministic outcome of the function, so it
        # is observed like an exception; anything else non-Exception
        # (KeyboardInterrupt, SystemExit, ...) is not an outcome: the caller
        # reports it and this side's run ends there.
        if not isinstance(x, Exception) and type(x).__name__ != 'PanicException': raise
        v = None; e = x
    finally:
        _on[0] = False
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
class _NonCanon(Exception): pass
def canon(x):
    # Canonical, order-independent form. int/str/bool/None reproduce repr;
    # float -> exact bits (NaN -> one token); list -> recursive, order
    # preserved (order is observable); dict -> sorted string keys (a map,
    # order is not observable). tuple -> recursive in parentheses (distinct
    # from list; a 1-tuple keeps its comma so it differs from its element).
    # namedtuple -> class name + field names + values, not identity, so
    # candidate and reference may each define their own class.
    # The plain types are matched exactly. A subclass of int, float, str,
    # list or dict (an enum, a Counter, an OrderedDict, ...) is read through
    # the base type's own methods and written under its class name, so it
    # agrees with the same class on the other side and never with the plain
    # type. Anything else (set, complex, custom object, other tuple
    # subclasses, non-string keys) raises _NonCanon -> Refused.
    t = type(x)
    if t is bool: return repr(x)
    if t is int:
        # An int past the interpreter's int-to-str digit limit has no decimal
        # repr; its hex form is exact and has no limit.
        try: return repr(x)
        except ValueError: return hex(x)
    if t is float: return 'nanq' if x != x else x.hex()
    if t is str: return repr(x)
    if x is None: return 'None'
    if t is list: return '[' + ', '.join(canon(e) for e in x) + ']'
    if t is tuple:
        return '(' + ', '.join(canon(e) for e in x) + (',' if len(x) == 1 else '') + ')'
    if isinstance(x, tuple):
        # Only a class made directly by namedtuple()/NamedTuple (bases exactly
        # (tuple,)); a further subclass may override __eq__ etc., so refuse it.
        _f = getattr(t, '_fields', None)
        if (t.__bases__ == (tuple,) and isinstance(_f, tuple) and len(_f) == len(x)
                and all(isinstance(k, str) for k in _f)):
            return t.__name__ + '(' + ', '.join(k + '=' + canon(v) for k, v in zip(_f, x)) + ')'
        raise _NonCanon()
    if t is dict:
        if not all(type(k) is str for k in x): raise _NonCanon()
        return '{{' + ', '.join(repr(k) + ': ' + canon(x[k]) for k in sorted(x)) + '}}'
    if isinstance(x, int): return t.__name__ + ':' + canon(int.__add__(x, 0))
    if isinstance(x, float): return t.__name__ + ':' + canon(float.__float__(x))
    if isinstance(x, str): return t.__name__ + ':' + canon(str.__getitem__(x, slice(None)))
    if isinstance(x, list): return t.__name__ + ':' + canon(list(list.__iter__(x)))
    if isinstance(x, dict):
        return t.__name__ + ':' + canon({{k: dict.__getitem__(x, k) for k in dict.keys(x)}})
    raise _NonCanon()
_m, _e, _o = _metered(load, {own:?}, {name:?})
if _o:
    _emit('__BUDGET__\tload\t{side}'); _stop()
# A load error is re-raised unchanged so its message still reaches the verdict.
if _e is not None: raise _e
# The cases are evaluated once from text rather than kept as a module-level
# literal in this file's code (keeps the driver's own code small for large
# value sets). The text is hex so any case source (quotes, escapes,
# lone-surrogate escapes) survives unchanged.
def _run_all(fn, cases):
    # One line per input, in order: V value, X exception class, B step budget
    # exhausted, N not a comparable value, U reading the value failed, Q the
    # function stopped the interpreter. N, U, Q and a B outside the tolerated
    # inputs end this side's run. The end line states how many inputs were
    # reported; a run without it did not finish.
    for i, args in enumerate(cases):
        try:
            v, e, out = _metered(fn, *args)
        except BaseException as x:
            _emit(f"{{i}}\tQ\t{{_hex(type(x).__name__)}}"); _emit(f"__END__\t{{i + 1}}"); _stop()
        if out:
            _emit(f"{{i}}\tB\t")
            if i >= _LARGE or i in _SOFT: continue
            _emit(f"__END__\t{{i + 1}}"); _stop()
        if e is not None:
            _emit(f"{{i}}\tX\t{{_hex(type(e).__name__)}}"); continue
        try: c = canon(v)
        except _NonCanon:
            _emit(f"{{i}}\tN\t"); _emit(f"__END__\t{{i + 1}}"); _stop()
        except Exception as x:
            _emit(f"{{i}}\tU\t{{_hex(type(x).__name__)}}"); _emit(f"__END__\t{{i + 1}}"); _stop()
        _emit(f"{{i}}\tV\t{{_hex(c)}}")
    _emit(f"__END__\t{{len(cases)}}")
_run_all(getattr(_m, {fn:?}), eval(bytes.fromhex('{cases_hex}').decode('utf-8')))
_stop()
"#,
        libm = libm,
        admis = admis,
        cand = cand,
        own = own,
        name = name,
        side = side,
        fn = spec.func,
        cases_hex = hex(format!("[{}]", cases_py.join(", ")).as_bytes()),
        budget = spec.budget,
        large_from = large_from,
        soft = if soft.is_empty() { "-1".to_string() } else { soft.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ") },
    )
}

/// Run the oracle. `candidate_path`/`reference_path` each define `spec.func`.
/// Float admissibility (AD-2) is enforced inside the driver as an AST allowlist
/// at source level; an inadmissible function surfaces as a `Refused` verdict.
/// Uses the default wall-clock safety limit (`DEFAULT_SAFETY_SECONDS`).
pub fn review(
    candidate_path: &Path,
    reference_path: &Path,
    spec: &ReviewSpec,
) -> Result<ReviewReceipt, SafetyStop> {
    review_limited(candidate_path, reference_path, spec, DEFAULT_SAFETY_SECONDS)
}

/// `review` with an explicit wall-clock safety limit in seconds. The limit only
/// decides whether a receipt exists at all; it never changes receipt bytes.
pub fn review_limited(
    candidate_path: &Path,
    reference_path: &Path,
    spec: &ReviewSpec,
    safety_secs: u64,
) -> Result<ReviewReceipt, SafetyStop> {
    review_with_inputs(candidate_path, reference_path, spec, &[], safety_secs)
}


/// Resolve every `str[in ...]` domain in `spec` from the candidate's own code
/// (0.24.0), in a separate evaluator run with the same package context the
/// review uses. A name that cannot be resolved, or that is not a non-empty
/// list, tuple, set or dict of strings, is an error naming the source: never a
/// silent fallback to unconstrained inputs.
fn resolve_value_sets(cand: &Path, spec: &ReviewSpec, safety_secs: u64) -> Result<ReviewSpec, String> {
    let sources: Vec<String> = spec
        .domains
        .iter()
        .filter_map(|d| match d {
            Domain::OneOf { source, values } if values.is_empty() => Some(source.clone()),
            _ => None,
        })
        .collect();
    if sources.is_empty() {
        return Ok(spec.clone());
    }
    const SCRIPT: &str = r#"
import sys, os, importlib, importlib.util as u
cand = sys.argv[1]
def _pkg_of(p):
    d = os.path.dirname(os.path.abspath(p)); parts = []
    while os.path.exists(os.path.join(d, '__init__.py')):
        parts.insert(0, os.path.basename(d)); d = os.path.dirname(d)
    return d, '.'.join(parts)
root, pkg = _pkg_of(cand)
sys.path.insert(0, root if pkg else os.path.dirname(os.path.abspath(cand)))
_cand_mod = []
def cand_module():
    if not _cand_mod:
        full = (pkg + '.cand') if pkg else 'cand'
        s = u.spec_from_file_location(full, cand); m = u.module_from_spec(s)
        if pkg: m.__package__ = pkg
        sys.modules[full] = m; s.loader.exec_module(m); _cand_mod.append(m)
    return _cand_mod[0]
for i, src in enumerate(sys.argv[2:]):
    try:
        mod, _, name = src.rpartition('.')
        if mod == '':
            m = cand_module()
        elif mod.startswith('.'):
            if not pkg: raise ImportError('a relative name needs the function to be inside a package')
            m = importlib.import_module(mod, pkg)
        else:
            m = importlib.import_module(mod)
        if not hasattr(m, name): raise AttributeError(f'{mod or "the module"} has no {name}')
        obj = getattr(m, name)
        if isinstance(obj, dict): vals = list(obj)
        elif isinstance(obj, (set, frozenset)): vals = sorted(obj) if all(isinstance(v, str) for v in obj) else list(obj)
        elif isinstance(obj, (list, tuple)): vals = list(obj)
        else: raise TypeError(f'is a {type(obj).__name__}, not a list, tuple, set or dict')
        if not vals: raise ValueError('is empty')
        bad = [v for v in vals if not isinstance(v, str)]
        if bad: raise TypeError(f'holds a {type(bad[0]).__name__}, not only strings')
        seen = set(); out = []
        for v in vals:
            if v not in seen: seen.add(v); out.append(v)
        print('__VS__\t%d\tOK\t%s' % (i, ','.join(v.encode('utf-8', 'surrogatepass').hex() for v in out)))
    except Exception as e:
        print('__VS__\t%d\tERR\t%s: %s' % (i, type(e).__name__, str(e).replace('\n', ' ')))
"#;
    // A private directory, removed when this function returns.
    let dir = PrivateDir::new().map_err(|e| format!("value-set resolver: {e}"))?;
    let path = dir.0.join("equiv_vs.py");
    std::fs::write(&path, SCRIPT).map_err(|e| format!("value-set resolver: {e}"))?;
    let mut cmd = evaluator_command();
    cmd.arg(&path).arg(cand).args(&sources).env("PYTHONHASHSEED", "0").env("PYTHONDONTWRITEBYTECODE", "1");
    let out = run_watched(cmd, std::time::Duration::from_secs(safety_secs));
    drop(dir);
    let out = out
        .map_err(|e| format!("python3 not runnable: {e}"))?
        .ok_or_else(|| format!("resolving the input domains {} ran past {safety_secs}s", sources.join(", ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut resolved: Vec<Option<Vec<String>>> = vec![None; sources.len()];
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("__VS__\t") else { continue };
        let mut f = rest.splitn(3, '\t');
        let (Some(i), Some(kind), Some(body)) = (f.next(), f.next(), f.next()) else { continue };
        let Ok(i) = i.parse::<usize>() else { continue };
        if i >= sources.len() {
            continue;
        }
        match kind {
            "OK" => {
                let mut vals = Vec::new();
                for h in body.split(',') {
                    let bytes: Option<Vec<u8>> =
                        (0..h.len()).step_by(2).map(|k| h.get(k..k + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect();
                    match bytes.and_then(|b| String::from_utf8(b).ok()) {
                        Some(v) => vals.push(v),
                        None => return Err(format!("input domain str[in {}]: a value is not valid Unicode text", sources[i])),
                    }
                }
                resolved[i] = Some(vals);
            }
            _ => return Err(format!("could not resolve the input domain str[in {}]: {body}", sources[i])),
        }
    }
    let mut out_spec = spec.clone();
    let mut k = 0;
    for d in out_spec.domains.iter_mut() {
        if let Domain::OneOf { source, values } = d {
            if values.is_empty() {
                match resolved.get(k).cloned().flatten() {
                    Some(v) => *values = v,
                    None => {
                        let err = String::from_utf8_lossy(&out.stderr);
                        let last = err.lines().last().unwrap_or("no output").to_string();
                        return Err(format!("could not resolve the input domain str[in {source}]: {last}"));
                    }
                }
                k += 1;
            }
        }
    }
    Ok(out_spec)
}


/// What the inputs covered of each value set (see `value_set_coverage`).
fn value_set_tried(spec: &ReviewSpec, cases: &[Vec<PyVal>]) -> ValueSetCoverage {
    value_set_coverage(spec, cases)
}

/// `review_limited` plus inputs harvested from the project's own code (see
/// `harvest`). They are bound into `spec_sha256` and counted in the receipt;
/// with none, the receipt is byte-identical to `review_limited`'s.
pub fn review_with_inputs(
    candidate_path: &Path,
    reference_path: &Path,
    spec: &ReviewSpec,
    harvested: &[Slots],
    safety_secs: u64,
) -> Result<ReviewReceipt, SafetyStop> {
    let cand_src = std::fs::read(candidate_path).unwrap_or_default();
    let ref_src = std::fs::read(reference_path).unwrap_or_default();
    let cand_text = String::from_utf8_lossy(&cand_src);
    let ref_text = String::from_utf8_lossy(&ref_src);
    let harvested: Vec<Slots> =
        harvested.iter().filter(|h| h.len() == spec.args.len()).cloned().collect();
    // Value-set domains are resolved first; a list that cannot be resolved is
    // "could not check", with the reason, and no input is generated.
    let (resolved, unresolved) = match resolve_value_sets(candidate_path, spec, safety_secs) {
        Ok(s) => (s, None),
        Err(reason) => (spec.clone(), Some(reason)),
    };
    let spec = &resolved;
    let (cases, large_from, inputs, soft) = if unresolved.is_some() {
        (Vec::new(), 0, InputCounts::default(), Vec::new())
    } else {
        gen_cases_soft(spec, &[&cand_text, &ref_text], &harvested)
    };

    let ev = match unresolved.map(|r| Err(PyFail::Msg(r))).unwrap_or_else(|| {
        run_python(candidate_path, reference_path, spec, &cases, large_from, &soft, safety_secs)
    }) {
        Ok(v) => v,
        Err(PyFail::Msg(reason)) => Evaluated { verdict: ReviewVerdict::Error { reason }, python: String::new(), exceptions: 0, disagreements: Vec::new() },
        Err(PyFail::Stopped) => {
            return Err(SafetyStop { func: spec.func.clone(), limit_secs: safety_secs });
        }
    };

    Ok(ReviewReceipt {
        candidate_sha256: sha256(&cand_src),
        reference_sha256: sha256(&ref_src),
        spec_sha256: sha256(&{
            let mut b = spec.canonical();
            if !harvested.is_empty() {
                b.extend(harvest::canonical(&harvested));
            }
            b
        }),
        checker_version: env!("CARGO_PKG_VERSION").into(),
        budget: spec.budget,
        python: ev.python,
        seed: spec.seed,
        inputs,
        domains: domains_note_tried(spec, &value_set_tried(spec, &cases)).unwrap_or_default(),
        exceptions: ev.exceptions,
        // The receipt itself carries the evidence level, so a thin verdict
        // cannot be re-read as a full one later (same frozen field, new value).
        tier: {
            let base = if spec.same_host { "tested-same-host" } else { "tested" };
            match ev.verdict {
                ReviewVerdict::Equivalent { n, .. } if thin_evidence(n, ev.exceptions).is_some() => format!("{base}-thin"),
                _ => base.into(),
            }
        },
        verdict: ev.verdict,
        disagreements: ev.disagreements,
    })
}

/// One line of provenance, the same wording everywhere a receipt is shown.
pub fn provenance_note(r: &ReviewReceipt) -> String {
    let i = &r.inputs;
    format!(
        "python {}, seed {}; inputs {} corner + {} literal + {} random{}{}; {} raised on both sides; tier {}",
        if r.python.is_empty() { "unknown" } else { &r.python },
        r.seed,
        i.corner,
        i.literal,
        i.random,
        if i.harvested > 0 { format!(" + {} from the project's code", i.harvested) } else { String::new() },
        if r.domains.is_empty() {
            String::new()
        } else {
            format!(
                "; inside domains {}{}",
                r.domains,
                if i.out_of_domain > 0 {
                    format!(" ({} source/harvested values outside them skipped)", i.out_of_domain)
                } else {
                    String::new()
                }
            )
        },
        r.exceptions,
        if r.tier.starts_with("tested-same-host") {
            format!("{} (SAME HOST ONLY: floats compared bit-exactly on this machine; another machine or maths library may give a different verdict)", r.tier)
        } else {
            r.tier.clone()
        }
    )
}

// ---------- PR review loop: base vs head ----------

/// One function to check in a PR: its new (head) source vs its base source.
pub struct PrCheck {
    pub name: String,
    pub head_path: std::path::PathBuf,
    pub base_path: std::path::PathBuf,
    pub spec: ReviewSpec,
    /// A twin check (0.28.0): `head_path` and `base_path` are two files of the
    /// PR's own tree (candidate, reference), named here for the comment.
    pub twin: Option<(String, String)>,
}

pub struct ReviewItem {
    pub name: String,
    /// The receipt, or the safety stop that prevented one (no receipt exists).
    pub outcome: Result<ReviewReceipt, SafetyStop>,
    /// Set for a twin check: (candidate file, reference file).
    pub twin: Option<(String, String)>,
}

/// Run every check (head = candidate, base = reference). Returns the items
/// plus a report code: 0 all equivalent, 1 any divergence, 2 nothing diverged
/// but at least one function could not be checked. Which codes block a merge
/// is the wrapper's policy (the Action fails on 1 by default and on 2 only
/// with `fail-on-not-checked: "true"`; backlog 0.3c).
pub fn run_pr(checks: &[PrCheck]) -> (Vec<ReviewItem>, i32) {
    run_pr_limited(checks, DEFAULT_SAFETY_SECONDS)
}

/// `run_pr` with an explicit per-function wall-clock safety limit. A function
/// that hits it is reported as could-not-check; the others still run.
pub fn run_pr_limited(checks: &[PrCheck], safety_secs: u64) -> (Vec<ReviewItem>, i32) {
    run_pr_with_inputs(checks, &[], safety_secs)
}

/// `run_pr_limited` with harvested inputs per check (`harvested[i]` belongs to
/// `checks[i]`; missing entries mean none).
pub fn run_pr_with_inputs(checks: &[PrCheck], harvested: &[Vec<Slots>], safety_secs: u64) -> (Vec<ReviewItem>, i32) {
    let mut items = Vec::new();
    let mut any_cex = false;
    let mut any_err = false;
    for (k, c) in checks.iter().enumerate() {
        // head is the candidate (the PR's new code), base is the reference.
        let h: &[Slots] = harvested.get(k).map(Vec::as_slice).unwrap_or(&[]);
        let outcome = review_with_inputs(&c.head_path, &c.base_path, &c.spec, h, safety_secs);
        match &outcome {
            Ok(r) => match &r.verdict {
                ReviewVerdict::Counterexample { .. } => any_cex = true,
                ReviewVerdict::Error { .. } | ReviewVerdict::Refused { .. } => any_err = true,
                ReviewVerdict::Equivalent { .. } => {}
            },
            Err(_) => any_err = true,
        }
        items.push(ReviewItem { name: c.name.clone(), outcome, twin: c.twin.clone() });
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
        return {'int':'int','str':'str','float':'float','dict':'dict','Dict':'dict','bool':'bool'}.get(a.id)
    if isinstance(a, ast.Subscript):
        base = a.value.id if isinstance(a.value, ast.Name) else None
        if base in ('list','List') and isinstance(a.slice, ast.Name) and a.slice.id == 'int':
            return 'list[int]'
        if base in ('list','List') and isinstance(a.slice, ast.Name) and a.slice.id == 'str':
            return 'list[str]'
        if base in ('dict','Dict'):
            return 'dict'
    return None
def simple(fn):
    # Positional defaults are allowed: every parameter still needs a supported
    # hint and every argument is generated, so the omitted-argument path is the
    # same default on both sides (a changed default is reported separately).
    a = fn.args
    return not (a.vararg or a.kwarg or a.kwonlyargs or a.posonlyargs or a.kw_defaults)
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
    if [ast.dump(d) for d in bnode.args.defaults] != [ast.dump(d) for d in hnode.args.defaults]:
        print('NOTCHECKED\t' + name + '\tdefault value changed; generated inputs pass every argument, so a default is never exercised'); continue
    sig, missing = [], []
    for a in hnode.args.args:
        t = ann(a.annotation)
        if t is None: missing.append(a.arg)
        else: sig.append(t)
    if not missing:
        print('CHECKABLE\t' + name + '\t' + ','.join(sig))
    else:
        # Types are never guessed (DECISIONS #20): name what needs a hint.
        print('NOTCHECKED\t' + name + '\tno supported type hint on ' + ', '.join('`' + m + '`' for m in missing)
              + ' (int, str, float, bool, dict, list[int] or list[str]); add one to check it')
"#;

/// Environment variables the Python evaluator may inherit. Everything else is
/// dropped, so code under review cannot read credentials equiv was handed
/// (`EQUIV_SIGNING_KEY`, `GH_TOKEN`, the Actions OIDC request token, cloud
/// keys). An allowlist, because a list of secret names is never complete.
/// Exact names, then prefixes (`PYTHON*`, `LC_*`). This removes credentials
/// from the evaluator's environment; it is not a sandbox: the evaluated code
/// still has the filesystem and network of the machine it runs on.
pub const EVALUATOR_ENV_ALLOW: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TEMP", "TMP", "LANG", "LANGUAGE", "TZ",
    "VIRTUAL_ENV", "CONDA_PREFIX", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH",
    // Windows: Python does not start without some of these.
    "SYSTEMROOT", "SystemRoot", "WINDIR", "windir", "PATHEXT", "COMSPEC", "ComSpec", "USERPROFILE",
    "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "SYSTEMDRIVE", "HOMEDRIVE", "HOMEPATH",
];
pub const EVALUATOR_ENV_ALLOW_PREFIX: &[&str] = &["PYTHON", "LC_"];
/// Interpreter switches that change what the reviewed code does (asserts
/// stripped, warnings raised as errors, a different int-to-str limit) without
/// being recorded in the receipt. They are not passed on, so the same review
/// gives the same verdict whatever the caller's shell has set. Import paths
/// (`PYTHONPATH`, `PYTHONHOME`) are still passed: reviews rely on them.
pub const EVALUATOR_ENV_DROP: &[&str] =
    &["PYTHONOPTIMIZE", "PYTHONWARNINGS", "PYTHONDEVMODE", "PYTHONINTMAXSTRDIGITS"];

fn evaluator_command() -> Command {
    let mut cmd = Command::new("python3");
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        let Some(name) = k.to_str() else { continue };
        if EVALUATOR_ENV_DROP.contains(&name) {
            continue;
        }
        if EVALUATOR_ENV_ALLOW.contains(&name) || EVALUATOR_ENV_ALLOW_PREFIX.iter().any(|p| name.starts_with(p)) {
            cmd.env(&k, &v);
        }
    }
    cmd
}

/// Detect which module-level functions changed between `base_src` and
/// `head_src` and classify each (close the manifest bypass, issue #1). The
/// invariant: a changed function is never silently dropped. Unchanged functions
/// are simply absent.
pub fn detect_changed_functions(base_src: &str, head_src: &str) -> Result<Vec<Detected>, String> {
    // A private directory per call, removed when this function returns.
    let private = PrivateDir::new()?;
    let dir = &private.0;
    let bpath = dir.join("equiv_detb.py");
    let hpath = dir.join("equiv_deth.py");
    let spath = dir.join("equiv_dets.py");
    std::fs::write(&bpath, base_src).map_err(|e| e.to_string())?;
    std::fs::write(&hpath, head_src).map_err(|e| e.to_string())?;
    std::fs::write(&spath, DETECTOR_PY).map_err(|e| e.to_string())?;
    let out = evaluator_command()
        .arg(&spath)
        .arg(&bpath)
        .arg(&hpath)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .map_err(|e| format!("python3 not runnable: {e}"));
    drop(private);
    let out = out?;
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
    let mut s = render_markdown_counting(items, signer, unchecked.len());
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
/// Takes the receipt itself: a safety stop has none, so it cannot be attested.
pub fn intoto_statement(file: &str, name: &str, receipt: &ReviewReceipt) -> String {
    use equiv_core::attest::{statement, ReviewPredicate};
    let (verdict, detail) = match &receipt.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => (
            "equivalent".to_string(),
            format!(
                "agreed on {n} cases{}; {}{}",
                not_judged_note(*not_judged),
                provenance_note(receipt),
                thin_note(receipt).map(|t| format!("; {t}")).unwrap_or_default()
            ),
        ),
        ReviewVerdict::Counterexample { input, candidate, reference, .. } => (
            "counterexample".to_string(),
            format!("diverges at {input}: this={candidate} base={reference}; {}", disagreements_note(&receipt.verdict)),
        ),
        ReviewVerdict::Error { reason } => ("error".to_string(), reason.clone()),
        ReviewVerdict::Refused { reason } => ("refused".to_string(), reason.clone()),
    };
    let receipt_id = hex(&receipt.sha256());
    let predicate = ReviewPredicate {
        verdict: &verdict,
        detail: &detail,
        receipt_id_hex: &receipt_id,
        checker: "equiv-review",
        checker_version: env!("CARGO_PKG_VERSION"),
    };
    statement(
        &format!("{file}::{name}"),
        &hex(&receipt.candidate_sha256),
        &predicate.to_json(),
    )
}

/// Sticky-comment marker so re-runs update one comment.
pub const MARKER: &str = "<!-- equiv-review -->";

/// Render the PR review as a GitHub-flavoured Markdown comment. `signer`, if
/// present, is the hex ed25519 public key that signed the receipts, shown so
/// reviewers/auditors can confirm it is the org's key.
pub fn render_markdown(items: &[ReviewItem], signer: Option<&str>) -> String {
    render_markdown_counting(items, signer, 0)
}

/// `render_markdown` whose headline also counts the changed functions that
/// auto-detection could not check (backlog 0.3c): a reader must see "not
/// checked" on the first line, never a green headline over an unchecked list.
fn render_markdown_counting(items: &[ReviewItem], signer: Option<&str>, unchecked_n: usize) -> String {
    let mut diverged = 0;
    let mut errored = 0;
    let mut thin = 0;
    for it in items {
        match &it.outcome {
            Ok(r) => match r.verdict {
                ReviewVerdict::Counterexample { .. } => diverged += 1,
                ReviewVerdict::Error { .. } | ReviewVerdict::Refused { .. } => errored += 1,
                ReviewVerdict::Equivalent { .. } => {
                    if thin_note(r).is_some() {
                        thin += 1;
                    }
                }
            },
            Err(_) => errored += 1,
        }
    }
    // A thin "equivalent" must be visible on the first line too, never folded
    // into a plain pass headline.
    let thin_more = match thin {
        0 => String::new(),
        1 => " 1 equivalent result rests on thin evidence (few inputs returned a value).".to_string(),
        n => format!(" {n} equivalent results rest on thin evidence (few inputs returned a value)."),
    };
    let total = items.len();
    let fns = |n: usize| if n == 1 { "function" } else { "functions" };
    let more = match unchecked_n {
        0 => String::new(),
        1 => " 1 changed function was not checked (listed below).".to_string(),
        n => format!(" {n} changed functions were not checked (listed below)."),
    };
    let headline = if diverged > 0 {
        format!("{diverged} of {total} checked {} changed behaviour.{more}{thin_more}", fns(total))
    } else if errored > 0 {
        format!("{errored} of {total} {} could not be checked.{more}{thin_more}", fns(total))
    } else if total == 0 && unchecked_n > 0 {
        // Nothing ran: this must never read as a pass (silence is not green).
        format!(
            "Nothing was checked: {unchecked_n} changed {} could not be checked (listed below).",
            fns(unchecked_n)
        )
    } else {
        let verb = if total == 1 { "function: behaviour" } else { "functions: behaviour" };
        format!("{total} checked {verb} preserved against base.{more}{thin_more}")
    };

    let mut s = String::new();
    s.push_str(MARKER);
    s.push('\n');
    s.push_str("## equiv review\n\n");
    s.push_str(&headline);
    s.push('\n');
    // Nothing ran: no result table and no receipts block, so the comment cannot
    // look like a pass with empty evidence; the "changed, not checked" list
    // that follows is the whole report.
    if items.is_empty() {
        s.push('\n');
        return finish_markdown(s);
    }
    s.push_str("\n| function | result | detail |\n|---|---|---|\n");
    for it in items {
        let r = match &it.outcome {
            Ok(r) => r,
            Err(stop) => {
                s.push_str(&format!(
                    "| `{}` | not checked | `{}` |\n",
                    it.name,
                    stop.reason().replace('`', "")
                ));
                continue;
            }
        };
        let (v, detail) = match &r.verdict {
            ReviewVerdict::Equivalent { n, not_judged } => {
                let base =
                    format!("{n}/{n} generated inputs agree{} ({})", not_judged_note(*not_judged), provenance_note(r));
                match thin_note(r) {
                    Some(t) => ("equivalent (thin evidence)".to_string(), format!("{base}; {t}")),
                    None => ("equivalent".to_string(), base),
                }
            }
            ReviewVerdict::Counterexample { input, candidate, reference, .. } => (
                "DIVERGES".to_string(),
                match &it.twin {
                    Some((a, b)) => format!(
                        "input `{input}`: `{a}` returns `{candidate}`, `{b}` returns `{reference}`; {} ({})",
                        disagreements_note(&r.verdict).replace('`', ""),
                        provenance_note(r)
                    ),
                    None => format!(
                        "input `{input}`: this PR returns `{candidate}`, base returns `{reference}`; {} ({})",
                        disagreements_note(&r.verdict).replace('`', ""),
                        provenance_note(r)
                    ),
                },
            ),
            ReviewVerdict::Error { reason } => {
                ("not checked".to_string(), format!("`{}`", reason.replace('`', "")))
            }
            ReviewVerdict::Refused { reason } => {
                ("refused".to_string(), reason.replace('`', ""))
            }
        };
        // A twin check names both files of the PR's tree it compared.
        let name = match &it.twin {
            Some((a, b)) => format!("`{}` (`{a}` vs `{b}`)", it.name),
            None => format!("`{}`", it.name),
        };
        s.push_str(&format!("| {name} | {} | {} |\n", v, detail));
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
        match &it.outcome {
            Ok(r) => s.push_str(&format!("- `{}`: receipt-id {}\n", it.name, short(&r.sha256()))),
            Err(_) => s.push_str(&format!("- `{}`: no receipt (safety stop)\n", it.name)),
        }
    }
    s.push_str("\n</details>\n\n");
    finish_markdown(s)
}

/// The scope disclaimer that closes every comment.
fn finish_markdown(mut s: String) -> String {
    s.push_str(
        "Scope: behavioural equivalence on generated inputs only. This does not check \
         intent, architecture, security. A passing result means behaviour was preserved \
         on the tested inputs, not that the change is correct.\n",
    );
    s
}

/// Why `run_python` produced no verdict: a message (becomes a receipted
/// could-not-check) or the wall-clock safety stop (no receipt at all).
enum PyFail {
    Msg(String),
    Stopped,
}

impl From<String> for PyFail {
    fn from(m: String) -> Self {
        PyFail::Msg(m)
    }
}

/// Process-group ids of evaluators currently running, so an interrupted CLI
/// can take them down too (the evaluator runs in its own process group, out of
/// reach of the terminal's Ctrl-C). Fixed slots: readable from a signal handler.
#[cfg(unix)]
static LIVE_GROUPS: [std::sync::atomic::AtomicI32; 16] =
    [const { std::sync::atomic::AtomicI32::new(0) }; 16];

/// Kill every evaluator process group still running. Async-signal-safe (atomic
/// loads and `kill` only), for the CLI's SIGINT/SIGTERM/SIGHUP handler.
pub fn kill_live_evaluators() {
    #[cfg(unix)]
    for slot in &LIVE_GROUPS {
        let g = slot.load(std::sync::atomic::Ordering::SeqCst);
        if g > 0 {
            // SAFETY: plain syscall on a process-group id we created.
            unsafe { libc::kill(-g, libc::SIGKILL) };
        }
    }
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // SAFETY: plain syscall. The child leads its own group (process_group(0)),
        // and is not reaped yet, so the group id cannot have been reused.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    let _ = child.kill();
}

/// Run `cmd`, killing its whole process group if it (or anything holding its
/// output pipes) is still going after `limit`. `Ok(None)` = safety stop. The
/// limit is wall-clock: it only ever turns a check into "no receipt", it never
/// chooses between verdicts.
fn run_watched(
    mut cmd: Command,
    limit: std::time::Duration,
) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let deadline = Instant::now() + limit;
    let mut child = cmd.spawn()?;
    #[cfg(unix)]
    let slot = LIVE_GROUPS.iter().find(|s| {
        s.compare_exchange(
            0,
            child.id() as i32,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
    });
    // Drain both pipes on their own threads: a child writing more than a pipe
    // buffer would otherwise block and be mistaken for a long run.
    let (tx, rx) = std::sync::mpsc::channel::<(u8, Vec<u8>)>();
    for (k, pipe) in [
        (0u8, child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
        (1u8, child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
    ] {
        let tx = tx.clone();
        let mut pipe = pipe.expect("piped");
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = pipe.read_to_end(&mut b);
            let _ = tx.send((k, b));
        });
    }
    drop(tx);
    let (mut out, mut err, mut status) = (None, None, None);
    let mut nap = Duration::from_millis(1);
    let result = loop {
        if status.is_none() {
            status = child.try_wait().ok().flatten();
        }
        while let Ok((k, b)) = rx.try_recv() {
            if k == 0 { out = Some(b) } else { err = Some(b) }
        }
        if let (Some(st), Some(_), Some(_)) = (status, &out, &err) {
            break Some(std::process::Output {
                status: st,
                stdout: out.take().unwrap_or_default(),
                stderr: err.take().unwrap_or_default(),
            });
        }
        let now = Instant::now();
        if now >= deadline {
            kill_group(&mut child);
            let _ = child.wait();
            break None;
        }
        if let Ok((k, b)) = rx.recv_timeout(nap.min(deadline - now)) {
            if k == 0 { out = Some(b) } else { err = Some(b) }
        }
        nap = (nap * 2).min(Duration::from_millis(50));
    };
    #[cfg(unix)]
    if let Some(s) = slot {
        s.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(result)
}

/// A directory only this process can use, removed when dropped. The driver
/// files live here, so nothing else on the machine can pre-create, replace or
/// shadow them, and nothing is left behind. The name is unpredictable and
/// never reaches a receipt.
struct PrivateDir(std::path::PathBuf);

impl PrivateDir {
    fn new() -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut seed = [0u8; 16];
        let mut filled = false;
        #[cfg(unix)]
        {
            use std::io::Read;
            if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
                filled = f.read_exact(&mut seed).is_ok();
            }
        }
        if !filled {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mix = sha256(format!("{t}:{}:{:p}", std::process::id(), &seed).as_bytes());
            seed.copy_from_slice(&mix[..16]);
        }
        for _ in 0..16 {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("equiv_{}_{n}", hex(&seed)));
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            match b.create(&p) {
                Ok(()) => return Ok(PrivateDir(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("could not create a private working directory: {e}")),
            }
        }
        Err("could not create a private working directory".to_string())
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What one side reported for one input. Values and names stay in the hex
/// form they crossed in, so two sides are compared on exact bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Outcome {
    Value(String),
    Raised(String),
    Budget,
    NonCanon,
    Unreadable(String),
    Quit(String),
}

/// One side's whole run, as reported.
#[derive(Default)]
struct SideRun {
    python: String,
    ok: bool,
    stderr_last: String,
    refused: Option<String>,
    nobudget: Option<String>,
    load_budget: bool,
    outcomes: Vec<Outcome>,
    /// The count on the end line; None if the run never wrote one.
    end: Option<usize>,
    /// A line that is not part of the protocol: nothing can be concluded.
    malformed: bool,
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    if h.len() % 2 != 0 || !h.is_ascii() {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

/// The readable form of a hex payload (for the headline only; never compared).
fn shown(h: &str) -> String {
    unhex(h).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

fn parse_side(out: &std::process::Output) -> SideRun {
    let mut r = SideRun { ok: out.status.success(), ..Default::default() };
    r.stderr_last = String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("python error").to_string();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if r.end.is_some() {
            // Nothing may follow the end line.
            r.malformed = true;
            break;
        }
        if let Some(v) = line.strip_prefix("__PY__\t") {
            r.python = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("__REFUSED__\t") {
            r.refused.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("__NOBUDGET__\t") {
            r.nobudget.get_or_insert(v.to_string());
        } else if line.starts_with("__BUDGET__\tload\t") {
            r.load_budget = true;
        } else if let Some(v) = line.strip_prefix("__END__\t") {
            match v.parse::<usize>() {
                Ok(n) if n == r.outcomes.len() => r.end = Some(n),
                _ => {
                    r.malformed = true;
                    break;
                }
            }
        } else {
            let parts: Vec<&str> = line.split('\t').collect();
            let idx_ok = parts.len() == 3 && parts[0].parse::<usize>().ok() == Some(r.outcomes.len());
            let payload_ok = parts.len() == 3 && unhex(parts[2]).is_some();
            let o = match (idx_ok && payload_ok, parts.get(1).copied()) {
                (true, Some("V")) => Some(Outcome::Value(parts[2].to_string())),
                (true, Some("X")) => Some(Outcome::Raised(parts[2].to_string())),
                (true, Some("B")) => Some(Outcome::Budget),
                (true, Some("N")) => Some(Outcome::NonCanon),
                (true, Some("U")) => Some(Outcome::Unreadable(parts[2].to_string())),
                (true, Some("Q")) => Some(Outcome::Quit(parts[2].to_string())),
                _ => None,
            };
            match o {
                Some(o) => r.outcomes.push(o),
                None => {
                    r.malformed = true;
                    break;
                }
            }
        }
    }
    r
}

/// The number of possible inputs when every argument has a small closed
/// domain (a bool, an int range, a value set; no arguments = 1), else None.
/// Only such a function can be checked completely by a handful of calls.
fn closed_space(spec: &ReviewSpec) -> Option<u128> {
    let mut total: u128 = 1;
    for (i, ty) in spec.args.iter().enumerate() {
        let size: u128 = match (ty, dom(spec, i)) {
            (ArgType::Bool, _) => 2,
            (ArgType::Int, Domain::IntRange(lo, hi)) => {
                let (lo, hi) = (*lo as i128, *hi as i128);
                if hi < lo { 0 } else { (hi - lo + 1) as u128 }
            }
            (ArgType::Str, Domain::OneOf { values, .. }) => values.len() as u128,
            _ => return None,
        };
        total = total.checked_mul(size)?;
    }
    Some(total)
}

fn run_python(
    cand: &Path,
    refr: &Path,
    spec: &ReviewSpec,
    cases: &[Vec<PyVal>],
    large_from: usize,
    soft: &[usize],
    safety_secs: u64,
) -> Result<Evaluated, PyFail> {
    use std::time::{Duration, Instant};
    let dir = PrivateDir::new()?;
    let started = Instant::now();
    let limit = Duration::from_secs(safety_secs);
    // One interpreter per side, the reference first: it has finished before
    // any candidate code runs. Both runs share the one wall-clock limit.
    let run_side = |is_cand: bool| -> Result<SideRun, PyFail> {
        let src = driver_source(is_cand, cand, refr, spec, cases, large_from, soft);
        let driver = dir.0.join(if is_cand { "equiv_review_candidate.py" } else { "equiv_review_reference.py" });
        let mut f = std::fs::File::create(&driver).map_err(|e| e.to_string())?;
        f.write_all(src.as_bytes()).map_err(|e| e.to_string())?;
        drop(f);
        let mut cmd = evaluator_command();
        cmd.arg(&driver)
            // Harden for determinism: stable hashing, no bytecode files.
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("");
        let left = limit.checked_sub(started.elapsed()).filter(|d| !d.is_zero()).ok_or(PyFail::Stopped)?;
        let out = run_watched(cmd, left)
            .map_err(|e| format!("python3 not runnable: {e}"))?
            .ok_or(PyFail::Stopped)?;
        Ok(parse_side(&out))
    };
    let r = run_side(false)?;
    let c = run_side(true)?;
    drop(dir);

    // The evaluator names itself on its first line, before the admissibility
    // and budget checks, so every receipt carries it. Empty if it never ran.
    let python = if c.python.is_empty() { r.python.clone() } else { c.python.clone() };
    let done = |verdict: ReviewVerdict, exceptions: u32| Evaluated {
        verdict,
        python: python.clone(),
        exceptions,
        disagreements: Vec::new(),
    };
    // A side that could not run (load error, missing function): the candidate's
    // message first, as when both were loaded in that order.
    for s in [&c, &r] {
        if !s.ok {
            return Ok(done(ReviewVerdict::Error { reason: s.stderr_last.clone() }, 0));
        }
    }
    // The driver's AST allowlist refuses inadmissible float functions before any
    // case runs, signalled on its own line.
    for s in [&c, &r] {
        if let Some(reason) = &s.refused {
            return Ok(done(ReviewVerdict::Refused { reason: reason.clone() }, 0));
        }
        if let Some(found) = &s.nobudget {
            return Ok(done(
                ReviewVerdict::Error {
                    reason: format!(
                        "the step budget needs Python 3.12 or newer (sys.monitoring); found {found}"
                    ),
                },
                0,
            ));
        }
    }
    let budget = spec.budget;
    for (s, side) in [(&c, "candidate"), (&r, "reference")] {
        if s.malformed {
            let reason = format!(
                "the {side} run wrote a result line that could not be read; nothing can be concluded from this run"
            );
            return Ok(done(ReviewVerdict::Error { reason }, 0));
        }
        if s.load_budget {
            let reason = format!(
                "loading the {side} module exceeded the step budget of {budget} Python \
                 line/jump events; a non-terminating or very long run is not judged"
            );
            return Ok(done(ReviewVerdict::Error { reason }, 0));
        }
    }
    let mut not_judged: u32 = 0;
    let mut exceptions: u32 = 0;
    let mut judged: u32 = 0;
    let mut found: Vec<Disagreement> = Vec::new();
    // Every case is read, not just up to the first disagreement: the first is
    // the headline, the count goes in the receipt, the list is kept in order.
    let finish = |found: Vec<Disagreement>, judged: u32, exceptions: u32, stopped: String| {
        let first = found[0].clone();
        Evaluated {
            verdict: ReviewVerdict::Counterexample {
                input: first.input,
                candidate: first.candidate,
                reference: first.reference,
                judged,
                disagreeing: found.len() as u32,
                stopped,
            },
            python: python.clone(),
            exceptions,
            disagreements: found,
        }
    };
    let input_at = |i: usize| -> String {
        cases[i].iter().map(|v| v.display()).collect::<Vec<_>>().join(", ")
    };
    // A stop that ends the review at one input: a disagreement found before
    // it still stands as the verdict, with the stop stated; without one, the
    // stop is the verdict. The reason states budget, case, input and side,
    // never the number of events consumed (that may differ between Python
    // versions).
    macro_rules! stop_here {
        ($reason:expr, $short:expr) => {{
            if !found.is_empty() {
                return Ok(finish(found, judged, exceptions, $short));
            }
            return Ok(done(ReviewVerdict::Error { reason: $reason }, exceptions));
        }};
    }
    // The two reports are walked in case order. Both sides must have reported
    // the input being judged: a side whose report stops short (it left the
    // interpreter, or its result lines never arrived) ends the review there,
    // and what it did not report is never counted as agreement.
    for i in 0..cases.len() {
        let (co, ro) = match (c.outcomes.get(i), r.outcomes.get(i)) {
            (Some(a), Some(b)) => (a, b),
            (a, _) => {
                let side = if a.is_none() { "candidate" } else { "reference" };
                let input = input_at(i);
                stop_here!(
                    format!(
                        "the {side} run ended before reporting input {} of {} ({input}); a run that ends early is not judged",
                        i + 1,
                        cases.len()
                    ),
                    format!("the {side} run ended before reporting input ({input})")
                );
            }
        };
        let (oc, or) = (*co == Outcome::Budget, *ro == Outcome::Budget);
        if oc || or {
            if i >= large_from || soft.contains(&i) {
                not_judged += 1;
                continue;
            }
            let side = if oc && or { "candidate and reference" } else if oc { "candidate" } else { "reference" };
            let input = input_at(i);
            stop_here!(
                format!(
                    "the {side} exceeded the step budget of {budget} Python line/jump events \
                     at input ({input}); a non-terminating or very long run is not judged"
                ),
                format!("the {side} exceeded the step budget of {budget} at input ({input})")
            );
        }
        for (o, side) in [(co, "candidate"), (ro, "reference")] {
            if let Outcome::Quit(name) = o {
                let (input, name) = (input_at(i), shown(name));
                stop_here!(
                    format!(
                        "the {side} stopped the interpreter at input ({input}) ({name}); a function that exits is not judged"
                    ),
                    format!("the {side} stopped the interpreter at input ({input})")
                );
            }
        }
        if *co == Outcome::NonCanon || *ro == Outcome::NonCanon {
            let reason = "returns a value that is not JSON-structural (no stable cross-host form)".to_string();
            return Ok(done(ReviewVerdict::Refused { reason }, 0));
        }
        for (o, side) in [(co, "candidate"), (ro, "reference")] {
            if let Outcome::Unreadable(name) = o {
                let reason = format!(
                    "the {side} returns a value that could not be read at input ({}) ({} while reading it)",
                    input_at(i),
                    shown(name)
                );
                return Ok(done(ReviewVerdict::Refused { reason }, 0));
            }
        }
        judged += 1;
        let raised = |o: &Outcome| matches!(o, Outcome::Raised(_));
        let payload = |o: &Outcome| match o {
            Outcome::Value(h) | Outcome::Raised(h) => h.clone(),
            _ => String::new(),
        };
        let agree = if raised(co) || raised(ro) {
            // A side raised: the classes must match, or (parity mode) both
            // must have raised. Counted as an exception observed.
            let ok = co == ro || (spec.errors_agree && raised(co) && raised(ro));
            if ok {
                exceptions += 1;
            }
            ok
        } else {
            co == ro
        };
        if !agree {
            // Each side's outcome: its exception class if it raised, else its value.
            found.push(Disagreement {
                input: format!("({})", input_at(i)),
                candidate: shown(&payload(co)),
                reference: shown(&payload(ro)),
            });
        }
    }
    // Both sides must have finished: the end line, stating every input.
    for (s, side) in [(&c, "candidate"), (&r, "reference")] {
        if s.end != Some(cases.len()) {
            stop_here!(
                format!("the {side} run did not finish (no end line after {} inputs); a run that ends early is not judged", s.outcomes.len()),
                format!("the {side} run did not finish")
            );
        }
    }
    if !found.is_empty() {
        return Ok(finish(found, judged, exceptions, String::new()));
    }
    // n is the number of cases actually judged (boundary + literal + random +
    // the large corners that finished, deduplicated), not the requested random
    // count: an honest tally. not_judged is the number of large-magnitude
    // corners where a side ran out of budget; both are in the receipt.
    let n = judged;
    // Never claim more than the evidence shows: if every input made both
    // versions raise, only errors were compared and the function's normal
    // path was never reached. That is "could not check", not a pass. The one
    // exemption is a function whose whole input space is fewer than THIN_K
    // values (no arguments, a bool, a small range or value set): those calls
    // checked it completely. A short case list over an open space is not that.
    let complete = closed_space(spec).is_some_and(|s| s < THIN_K as u128);
    if n > 0 && exceptions >= n && (n >= THIN_K || !complete) {
        let reason = format!(
            "only errors were compared: all {n} generated inputs made both versions raise, so no input reached \
             a returned value; add inputs the function accepts (literal calls in the project's tests, or --inputs-from)"
        );
        return Ok(done(ReviewVerdict::Refused { reason }, exceptions));
    }
    // Inputs existed but every one ran past the step budget: nothing was
    // compared, so this is "could not check", never "agreed on all 0".
    if n == 0 && not_judged > 0 {
        let reason = format!(
            "no input was judged: all {not_judged} inputs ran past the step budget of {} Python line/jump events; \
             raise it with --budget",
            spec.budget
        );
        return Ok(done(ReviewVerdict::Error { reason }, exceptions));
    }
    Ok(done(ReviewVerdict::Equivalent { n, not_judged }, exceptions))
}

/// What one evaluator run produced: the verdict plus the provenance the
/// receipt records (evaluator version, exceptions observed).
struct Evaluated {
    verdict: ReviewVerdict,
    python: String,
    exceptions: u32,
    disagreements: Vec<Disagreement>,
}

// Tests only: these pin properties of the private generators that the
// mutation audit (2026-10-30) found unasserted. No release code lives here.
#[cfg(test)]
mod generator_tests {
    use super::*;

    fn spec(args: Vec<ArgType>, seed: u64) -> ReviewSpec {
        ReviewSpec { func: "f".into(), args, n: 200, seed, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host: false }
    }

    #[test]
    fn surrogate_literals_escape_lone_units_and_match_valid_utf16() {
        assert_eq!(py_units_literal(&[0xD800]), "'\\ud800'");
        assert_eq!(py_units_literal(&[0x78, 0xDC80]), "'x\\udc80'");
        // a valid pair decodes to the same literal a String would give
        assert_eq!(py_units_literal(&[0xD83D, 0xDE00]), py_str_literal("😀"));
        assert_eq!(py_units_literal(&[0x61, 0x27, 0x5C]), py_str_literal("a'\\"));
    }

    fn matches_type(v: &PyVal, t: ArgType) -> bool {
        matches!(
            (v, t),
            (PyVal::Int(_), ArgType::Int)
                | (PyVal::Str(_) | PyVal::StrUnits(_), ArgType::Str)
                | (PyVal::ListInt(_), ArgType::ListInt)
                | (PyVal::Float(_), ArgType::Float)
                | (PyVal::Dict(_), ArgType::Dict)
                | (PyVal::Bool(_), ArgType::Bool)
                | (PyVal::ListStr(_), ArgType::ListStr)
        )
    }

    // Every generated input matches the declared type of its slot. A
    // type-invalid input reaching both sides could manufacture a false
    // counterexample under the declared contract (audit FLAG 1).
    #[test]
    fn every_generated_input_matches_its_declared_type() {
        let src = "def f(a, b):\n    if a == 0xFF or b == \"port\":\n        return 1 << 19\n    return 2 ** 31\n";
        for args in [
            vec![ArgType::Int, ArgType::Int],
            vec![ArgType::Str, ArgType::Int],
            vec![ArgType::Int, ArgType::Str, ArgType::Float, ArgType::Dict, ArgType::ListInt],
            vec![ArgType::Str, ArgType::Str, ArgType::Float, ArgType::Float],
            vec![ArgType::Bool, ArgType::ListStr, ArgType::Int],
        ] {
            let cases = gen_cases(&spec(args.clone(), 1), &[src, src]);
            assert!(cases.len() > args.len() * 5, "too few cases for {args:?}: {}", cases.len());
            for c in &cases {
                assert_eq!(c.len(), args.len(), "arity of case {c:?}");
                for (v, t) in c.iter().zip(&args) {
                    assert!(matches_type(v, *t), "slot of type {t:?} received {v:?} (case {c:?})");
                }
            }
        }
    }

    // Without source literals, every int outside the O(n)-safe envelope is one
    // of the fixed large corners, and those all run after the random cases
    // (random arms, boundary corners and joint corners stay inside).
    #[test]
    fn generated_ints_stay_inside_the_envelope_without_literals() {
        let cases = gen_cases(&spec(vec![ArgType::Int, ArgType::Int], 1), &["def f(a, b):\n    return a\n"]);
        let n_large = cases
            .iter()
            .filter(|c| c.iter().any(|v| matches!(v, PyVal::Int(n) if n.unsigned_abs() > INT_ENVELOPE as u64)))
            .count();
        assert!(n_large > 0, "the large corners must be present");
        for (idx, c) in cases.iter().enumerate() {
            for v in c {
                if let PyVal::Int(n) = v {
                    if n.unsigned_abs() > INT_ENVELOPE as u64 {
                        assert!(LARGE_INT_CORNERS.contains(n), "{n} is outside the envelope and not a fixed corner");
                        assert!(idx >= cases.len() - n_large, "large corner {n} ran at {idx}, before the random cases");
                    }
                }
            }
        }
    }

    // The large corners are pinned, with their negatives, and never inside
    // the envelope (there they would belong to boundary_cases).
    #[test]
    fn large_int_corners_are_pinned() {
        assert_eq!(LARGE_INT_CORNERS.len(), 19);
        for &n in LARGE_INT_CORNERS {
            assert!(n.unsigned_abs() > INT_ENVELOPE as u64, "{n} is inside the envelope");
            if n != i64::MIN {
                assert!(LARGE_INT_CORNERS.contains(&-n), "{n} has no negative");
            }
        }
        for n in [2_147_483_648i64, 4_294_967_296, 562_949_953_421_313, 9_007_199_254_740_993, i64::MAX, i64::MIN] {
            assert!(LARGE_INT_CORNERS.contains(&n), "{n} missing");
        }
    }

    // The deterministic int corners are pinned: dropping one changes which
    // off-by-one bugs are reachable, silently.
    #[test]
    fn int_boundary_corners_are_pinned() {
        assert_eq!(boundary_cases(ArgType::Int), [0, 1, -1, 2, -2].map(PyVal::Int).to_vec());
    }

    // The random stream must not collapse for an even seed. xorshift has an
    // all-zero fixed point; the seed is made odd before use, so seed 2 runs
    // a full stream like seed 1 (the suite and the CLI default otherwise only
    // ever use seed 1).
    #[test]
    fn even_seed_runs_a_full_random_stream() {
        let src = "def f(a, b):\n    return a + b\n";
        let args = vec![ArgType::Int, ArgType::Int];
        let n1 = gen_cases(&spec(args.clone(), 1), &[src]).len();
        let n2 = gen_cases(&spec(args.clone(), 2), &[src]).len();
        let n4 = gen_cases(&spec(args, 4), &[src]).len();
        assert!(n1 > 150, "seed 1 produced only {n1} cases");
        assert!(n2 > 150, "seed 2 produced only {n2} cases: the random stream collapsed");
        assert!(n4 > 150, "seed 4 produced only {n4} cases: the random stream collapsed");
    }

    // Different seeds give different random cases (the seed is live, not
    // decorative), and the same seed gives the same list (determinism).
    #[test]
    fn seed_changes_random_cases_and_is_deterministic() {
        let src = "def f(a):\n    return a\n";
        let a = gen_cases(&spec(vec![ArgType::Int], 1), &[src]);
        let b = gen_cases(&spec(vec![ArgType::Int], 1), &[src]);
        let c = gen_cases(&spec(vec![ArgType::Int], 2), &[src]);
        assert_eq!(a, b, "same seed, same cases");
        assert_ne!(a, c, "a different seed must change the random cases");
    }

    // Neighbouring seeds must not alias (path A step 4 finding: `seed | 1`
    // ran seed 2 as seed 3 and seed 4 as seed 5), and odd seeds keep the
    // stream they had, so seed 1 and every odd-seed receipt are unchanged.
    #[test]
    fn neighbouring_seeds_give_different_streams_and_odd_seeds_are_unchanged() {
        let src = "def f(a):\n    return a\n";
        let cases = |seed| gen_cases(&spec(vec![ArgType::Int], seed), &[src]);
        for (even, odd) in [(0, 1), (2, 3), (4, 5), (100, 101)] {
            assert_ne!(cases(even), cases(odd), "seed {even} ran the same stream as seed {odd}");
        }
        let mut r = seed_rng(7);
        let mut legacy = Rng(7);
        for _ in 0..5 {
            assert_eq!(r.next(), legacy.next(), "an odd seed must keep its pre-0.21 stream");
        }
    }

    // 0.17.0: random strings mix case, digits and control characters, and
    // multi-element lists carry them together (the uutils sort replay found
    // nothing because they never did). Length stays <= 8 characters.
    #[test]
    fn random_strings_mix_case_digits_and_control_characters() {
        let src = "def f(xs):\n    return xs\n";
        let mut sp = spec(vec![ArgType::ListStr], 1);
        sp.n = 2000; // the default
        let cases = gen_cases(&sp, &[src]);
        let lists: Vec<&Vec<String>> = cases
            .iter()
            .filter_map(|c| match &c[0] {
                PyVal::ListStr(v) => Some(v),
                _ => None,
            })
            .collect();
        let all: Vec<&String> = lists.iter().flat_map(|v| v.iter()).collect();
        let upper = all.iter().filter(|s| s.chars().any(|c| c.is_ascii_uppercase())).count();
        let ctrl = all.iter().filter(|s| s.chars().any(|c| matches!(c, '\t' | '\r' | '\n'))).count();
        let numeric = |s: &&String| !s.is_empty() && s.trim().trim_start_matches(['-', '+']).chars().all(|c| c.is_ascii_digit());
        let lists_with_two_numbers = lists.iter().filter(|v| v.iter().filter(numeric).count() >= 2).count();
        let lists_mixing_case = lists
            .iter()
            .filter(|v| v.iter().any(|s| s.chars().any(|c| c.is_ascii_uppercase())) && v.len() >= 2)
            .count();
        assert!(upper > 100, "only {upper} strings with a capital");
        assert!(ctrl > 50, "only {ctrl} strings with \\t, \\r or \\n");
        assert!(lists_with_two_numbers > 50, "only {lists_with_two_numbers} lists with two numeric strings");
        assert!(lists_mixing_case > 100, "only {lists_mixing_case} multi-element lists with a capital");
        let mut rng = Rng(3);
        for _ in 0..5000 {
            match gen_arg(&mut rng, ArgType::Str) {
                PyVal::Str(s) => assert!(s.chars().count() <= 8, "{s:?} longer than 8"),
                _ => unreachable!(),
            }
        }
    }

    // 0.17.0: string literals are decoded, not mangled. "\t" stays a tab,
    // "é" is kept, \x/\u escapes resolve, a raw string keeps its backslash.
    #[test]
    fn string_literals_keep_escapes_and_non_ascii() {
        let src = r#"if s == '\t' or s == "é" or s == '\x41\u00e9' or re.match(r'\d+', s) or s == 'it\'s': pass"#;
        let got = scan_str_literals(src);
        for want in ["\t", "é", "Aé", "\\d+", "it's"] {
            assert!(got.iter().any(|g| g == want), "missing {want:?} in {got:?}");
        }
        assert!(!got.iter().any(|g| g == "t"), "an escape lost its backslash: {got:?}");
    }
}
