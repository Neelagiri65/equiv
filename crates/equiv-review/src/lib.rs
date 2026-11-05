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
        v
    }
}

pub mod harvest;
pub use harvest::Slots;

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
    Bool(bool),
    ListStr(Vec<String>),
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
        ArgType::Bool => PyVal::Bool(false),
        ArgType::ListStr => PyVal::ListStr(Vec::new()),
    }
}

// String corners the random generator (len<=8, small alphabet) rarely or never reaches. Grouped by
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
    // numeric text: a leading sign (zfill vs rjust, isdecimal vs int()), a
    // decimal point and an exponent (float() accepts both), whitespace-padded
    // digits (int() and float() ignore the padding), and signed / decimal zero
    "-1", "+1", "1.5", "1e3", " 7 ", "-0", "+0", "0.0",
];

// Deterministic corner inputs the seeded random generator reaches rarely or
// never: the exact shapes where refactors break. All stay inside the O(n)-safe
// envelope so a linear reference cannot hang on them.
fn boundary_cases(ty: ArgType) -> Vec<PyVal> {
    match ty {
        // Off-by-one and sign corners (0, +/-1, +/-2) within the envelope.
        ArgType::Int => [0, 1, -1, 2, -2].into_iter().map(PyVal::Int).collect(),
        // Shapes the random generator (len<=8) rarely reaches: empty,
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

// String literals in the source, decoded. Escapes are resolved (\n, \t, \r,
// \\, quotes, \xHH, \uXXXX, \UXXXXXXXX, \0) and non-ASCII characters are
// kept, so a branch on "\t" or "é" is tested with that exact text. Before
// 0.17.0 escapes lost their backslash ("\n" became "n") and non-ASCII bytes
// were dropped. A raw-string prefix (r"..") keeps its backslashes. Still a
// token scan, not a parser: a superset, which only ever adds inputs.
fn scan_str_literals(src: &str) -> Vec<String> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let q = cs[i];
        if q == '\'' || q == '"' {
            let raw = i > 0 && matches!(cs[i - 1], 'r' | 'R');
            let mut j = i + 1;
            let mut s = String::new();
            let mut closed = false;
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
                    let hex_at = |from: usize, len: usize| -> Option<char> {
                        let t: String = cs.get(from..from + len)?.iter().collect();
                        char::from_u32(u32::from_str_radix(&t, 16).ok()?)
                    };
                    let (ch, used) = match e {
                        'n' => (Some('\n'), 2),
                        't' => (Some('\t'), 2),
                        'r' => (Some('\r'), 2),
                        '0' => (Some('\0'), 2),
                        'x' => (hex_at(j + 2, 2), 4),
                        'u' => (hex_at(j + 2, 4), 6),
                        'U' => (hex_at(j + 2, 8), 10),
                        '\n' => (None, 2), // line continuation
                        other => (Some(other), 2),
                    };
                    if let Some(ch) = ch {
                        s.push(ch);
                    }
                    j += used;
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
            if closed && s.chars().count() <= 64 {
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
        ArgType::Bool => Vec::new(),
        // Each string literal as a singleton list.
        ArgType::ListStr => scan_str_literals(src).into_iter().map(|k| PyVal::ListStr(vec![k])).collect(),
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
    let mut soft: Vec<usize> = Vec::new();
    let mut cases: Vec<Vec<PyVal>> = Vec::new();
    let mut counts = InputCounts::default();
    let mut seen: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    fn key(c: &[PyVal]) -> Vec<String> {
        c.iter().map(|v| v.to_py()).collect()
    }

    if !spec.args.is_empty() {
        let base: Vec<PyVal> = spec.args.iter().map(|t| default_val(*t)).collect();
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
                if seen.insert(key(&c)) {
                    soft.push(cases.len());
                    cases.push(c);
                    counts.harvested += 1;
                }
            }
        }
        for h in harvested {
            for (i, v) in h.iter().enumerate() {
                if let Some(v) = v.as_ref().filter(|v| harvest::inside_envelope(v)) {
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
        for (i, ty) in spec.args.iter().enumerate() {
            // Order is unchanged from before counting existed (corners, then
            // literals, per argument); only the attribution is recorded.
            let n_corner = boundary_cases(*ty).len();
            let mut vals = boundary_cases(*ty);
            for s in sources {
                vals.extend(literal_inputs(s, *ty));
            }
            for (k, v) in vals.into_iter().enumerate() {
                let mut c = base.clone();
                c[i] = v;
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
        // that flag set to True. One flag at a time keeps the cost linear in
        // the number of bools; a divergence that needs two flags set at once
        // is still out of reach. Signatures without a bool add nothing.
        for fb in flag_bases(spec).into_iter().skip(1) {
            for (i, ty) in spec.args.iter().enumerate() {
                if *ty == ArgType::Bool {
                    continue;
                }
                let n_corner = boundary_cases(*ty).len();
                let mut vals = boundary_cases(*ty);
                for s in sources {
                    vals.extend(literal_inputs(s, *ty));
                }
                for (k, v) in vals.into_iter().enumerate() {
                    let mut c = fb.clone();
                    c[i] = v;
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
                            counts.corner += 1;
                        }
                    }
                }
            }
        }
    }

    if !spec.args.is_empty() {
        let base: Vec<PyVal> = spec.args.iter().map(|t| default_val(*t)).collect();
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
                        if seen.insert(key(&c)) {
                            cases.push(c);
                            counts.corner += 1;
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
            counts.random += 1;
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
                    if seen.insert(key(&c)) {
                        cases.push(c);
                        counts.corner += 1;
                    }
                }
            }
        }
        // Harvested values outside the envelope (a test's f(10**12)): here,
        // where a side running out of budget is "not judged at this input".
        let base: Vec<PyVal> = spec.args.iter().map(|t| default_val(*t)).collect();
        for h in harvested {
            if h.iter().all(Option::is_some) && !h.iter().flatten().all(harvest::inside_envelope) {
                let c: Vec<PyVal> = h.iter().flatten().cloned().collect();
                if seen.insert(key(&c)) {
                    cases.push(c);
                    counts.harvested += 1;
                }
            }
            for (i, v) in h.iter().enumerate() {
                if let Some(v) = v.as_ref().filter(|v| !harvest::inside_envelope(v)) {
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
    (cases, large_from, counts, soft)
}

// The default base (every argument at its default) followed by one base per
// bool argument with that flag set to True. Just the default base when the
// signature holds no bool, so other signatures generate exactly as before.
fn flag_bases(spec: &ReviewSpec) -> Vec<Vec<PyVal>> {
    let base: Vec<PyVal> = spec.args.iter().map(|t| default_val(*t)).collect();
    let mut out = vec![base.clone()];
    for (i, ty) in spec.args.iter().enumerate() {
        if *ty == ArgType::Bool {
            let mut b = base.clone();
            b[i] = PyVal::Bool(true);
            out.push(b);
        }
    }
    out
}

// ---------- verdict + receipt ----------

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
                ]),
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

fn driver_source(cand: &Path, refr: &Path, spec: &ReviewSpec, cases: &[Vec<PyVal>], large_from: usize, soft: &[usize]) -> String {
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
    // math.sqrt, comparisons, literals, a small set of exact builtins). Anything
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
    let libm = format!(
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
        print('__REFUSED__\t' + _w); _sys.exit(0)
"#,
        fn = spec.func,
        cand = cand,
        refr = refr,
    );
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
print('__PY__\t' + str(_sys.version_info[0]) + '.' + str(_sys.version_info[1]))
{libm}{admis}# Deterministic step budget (backlog 0.3, gate P3). Every LINE and JUMP event
# raised by code outside this driver file counts one step; past the budget the
# callback raises _Budget, and keeps raising on every later event, so user code
# that swallows it (even a bare except) cannot loop on. No clock is read.
if _sys.version_info < (3, 12):
    print('__NOBUDGET__\t' + _sys.version.split()[0]); _sys.exit(0)
_M = _sys.monitoring
_TOOL = 4
_EV = _M.events.LINE | _M.events.JUMP
_BUDGET = {budget}
# Cases from index _LARGE on are the large-magnitude int corners (outside
# the envelope). Exhausting the budget there is 'not judged at this input',
# printed as SKIP and counted; before it, exhaustion ends the review.
_LARGE = {large_from}
# Harvested inputs before the tail: also 'not judged' on budget exhaustion.
_SOFT = {{{soft}}}
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
    # string keys (a map, order is not observable). tuple -> recursive in
    # parentheses (distinct from list; a 1-tuple keeps its comma so it differs
    # from its element). namedtuple -> class name + field names + values, not
    # identity, so candidate and reference may each define their own class.
    # Anything else (set, complex, custom object, other tuple subclasses,
    # non-string keys) -> __NONCANON__, which the driver turns into Refused.
    if isinstance(x, bool): return repr(x)
    if isinstance(x, int): return repr(x)
    if isinstance(x, float): return 'nanq' if x != x else x.hex()
    if isinstance(x, str): return repr(x)
    if x is None: return 'None'
    if isinstance(x, list): return '[' + ', '.join(canon(e) for e in x) + ']'
    if type(x) is tuple:
        return '(' + ', '.join(canon(e) for e in x) + (',' if len(x) == 1 else '') + ')'
    if isinstance(x, tuple):
        # Only a class made directly by namedtuple()/NamedTuple (bases exactly
        # (tuple,)); a further subclass may override __eq__ etc., so refuse it.
        _f = getattr(type(x), '_fields', None)
        if (type(x).__bases__ == (tuple,) and isinstance(_f, tuple) and len(_f) == len(x)
                and all(isinstance(k, str) for k in _f)):
            return type(x).__name__ + '(' + ', '.join(k + '=' + canon(v) for k, v in zip(_f, x)) + ')'
        return '__NONCANON__'
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
        if i >= _LARGE or i in _SOFT:
            print(f"{{i}}\tSKIP"); continue
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
        # EQX: agreed, and a side raised (counted as an exception observed).
        print(f"{{i}}\t{{'EQX' if ok else 'NE'}}\t{{dc}}\t{{dr}}")
    else:
        print(f"{{i}}\t{{'EQ' if cv == rv else 'NE'}}\t{{cv}}\t{{rv}}")
"#,
        libm = libm,
        admis = admis,
        cand = cand,
        refr = refr,
        fn = spec.func,
        cases = cases_py.join(", "),
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
    let (cases, large_from, inputs, soft) = gen_cases_soft(spec, &[&cand_text, &ref_text], &harvested);

    let ev = match run_python(candidate_path, reference_path, spec, &cases, large_from, &soft, safety_secs) {
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
        verdict: ev.verdict,
        checker_version: env!("CARGO_PKG_VERSION").into(),
        budget: spec.budget,
        python: ev.python,
        seed: spec.seed,
        inputs,
        exceptions: ev.exceptions,
        tier: "tested".into(),
        disagreements: ev.disagreements,
    })
}

/// One line of provenance, the same wording everywhere a receipt is shown.
pub fn provenance_note(r: &ReviewReceipt) -> String {
    let i = &r.inputs;
    format!(
        "python {}, seed {}; inputs {} corner + {} literal + {} random{}; {} raised on both sides; tier {}",
        if r.python.is_empty() { "unknown" } else { &r.python },
        r.seed,
        i.corner,
        i.literal,
        i.random,
        if i.harvested > 0 { format!(" + {} from the project's code", i.harvested) } else { String::new() },
        r.exceptions,
        r.tier
    )
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
    /// The receipt, or the safety stop that prevented one (no receipt exists).
    pub outcome: Result<ReviewReceipt, SafetyStop>,
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
        items.push(ReviewItem { name: c.name.clone(), outcome });
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

fn evaluator_command() -> Command {
    let mut cmd = Command::new("python3");
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        let Some(name) = k.to_str() else { continue };
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
    let tag = hex(&sha256(format!("{base_src}\u{0}{head_src}").as_bytes()))[..16].to_string();
    let dir = std::env::temp_dir();
    let bpath = dir.join(format!("equiv_detb_{tag}.py"));
    let hpath = dir.join(format!("equiv_deth_{tag}.py"));
    let spath = dir.join(format!("equiv_dets_{tag}.py"));
    std::fs::write(&bpath, base_src).map_err(|e| e.to_string())?;
    std::fs::write(&hpath, head_src).map_err(|e| e.to_string())?;
    std::fs::write(&spath, DETECTOR_PY).map_err(|e| e.to_string())?;
    let out = evaluator_command()
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
            format!("agreed on {n} cases{}; {}", not_judged_note(*not_judged), provenance_note(receipt)),
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
    for it in items {
        match &it.outcome {
            Ok(r) => match r.verdict {
                ReviewVerdict::Counterexample { .. } => diverged += 1,
                ReviewVerdict::Error { .. } | ReviewVerdict::Refused { .. } => errored += 1,
                ReviewVerdict::Equivalent { .. } => {}
            },
            Err(_) => errored += 1,
        }
    }
    let total = items.len();
    let fns = |n: usize| if n == 1 { "function" } else { "functions" };
    let more = match unchecked_n {
        0 => String::new(),
        1 => " 1 changed function was not checked (listed below).".to_string(),
        n => format!(" {n} changed functions were not checked (listed below)."),
    };
    let headline = if diverged > 0 {
        format!("{diverged} of {total} checked {} changed behaviour.{more}", fns(total))
    } else if errored > 0 {
        format!("{errored} of {total} {} could not be checked.{more}", fns(total))
    } else if total == 0 && unchecked_n > 0 {
        // Nothing ran: this must never read as a pass (silence is not green).
        format!(
            "Nothing was checked: {unchecked_n} changed {} could not be checked (listed below).",
            fns(unchecked_n)
        )
    } else {
        let verb = if total == 1 { "function: behaviour" } else { "functions: behaviour" };
        format!("{total} checked {verb} preserved against base.{more}")
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
            ReviewVerdict::Equivalent { n, not_judged } => (
                "equivalent".to_string(),
                format!("{n}/{n} generated inputs agree{} ({})", not_judged_note(*not_judged), provenance_note(r)),
            ),
            ReviewVerdict::Counterexample { input, candidate, reference, .. } => (
                "DIVERGES".to_string(),
                format!(
                    "input `{input}`: this PR returns `{candidate}`, base returns `{reference}`; {} ({})",
                    disagreements_note(&r.verdict).replace('`', ""),
                    provenance_note(r)
                ),
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

fn run_python(
    cand: &Path,
    refr: &Path,
    spec: &ReviewSpec,
    cases: &[Vec<PyVal>],
    large_from: usize,
    soft: &[usize],
    safety_secs: u64,
) -> Result<Evaluated, PyFail> {
    let src = driver_source(cand, refr, spec, cases, large_from, soft);
    // Content-addressed driver path: unique per distinct review (no clobber
    // between concurrent runs), stable for the same review.
    let tag = hex(&sha256(src.as_bytes()))[..16].to_string();
    let driver = std::env::temp_dir().join(format!("equiv_review_{tag}.py"));
    let mut f = std::fs::File::create(&driver).map_err(|e| e.to_string())?;
    f.write_all(src.as_bytes()).map_err(|e| e.to_string())?;
    drop(f);

    let mut cmd = evaluator_command();
    cmd.arg(&driver)
        // Harden for determinism: stable hashing, no user site packages.
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("");
    let out = run_watched(cmd, std::time::Duration::from_secs(safety_secs))
        .map_err(|e| format!("python3 not runnable: {e}"))?
        .ok_or(PyFail::Stopped)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The evaluator names itself on its first line, before the admissibility
    // and budget checks, so every receipt carries it. Empty if it never ran.
    let python = stdout
        .lines()
        .find_map(|l| l.strip_prefix("__PY__\t"))
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let done = |verdict: ReviewVerdict, exceptions: u32| Evaluated {
        verdict,
        python: python.clone(),
        exceptions,
        disagreements: Vec::new(),
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().last().unwrap_or("python error").to_string();
        return Ok(done(ReviewVerdict::Error { reason: last }, 0));
    }
    // The driver's AST allowlist refuses inadmissible float functions before any
    // case runs, signalled on its own line.
    for line in stdout.lines() {
        if let Some(reason) = line.strip_prefix("__REFUSED__\t") {
            return Ok(done(ReviewVerdict::Refused { reason: reason.to_string() }, 0));
        }
        if let Some(found) = line.strip_prefix("__NOBUDGET__\t") {
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
    for line in stdout.lines() {
        // Lines arrive in case order and the driver stops at the first budget
        // exhaustion. A disagreement found before it still stands as the
        // verdict, with the stop stated; without one, the stop is the verdict.
        // The reason states budget, case, input and side, never the number of
        // events consumed (that may differ between Python versions).
        if let Some(rest) = line.strip_prefix("__BUDGET__\t") {
            let (at, side) = rest.split_once('\t').unwrap_or((rest, "candidate"));
            let (reason, short) = if at == "load" {
                (
                    format!(
                        "loading the {side} module exceeded the step budget of {budget} Python \
                         line/jump events; a non-terminating or very long run is not judged"
                    ),
                    format!("step budget of {budget} exceeded loading the {side}"),
                )
            } else {
                let idx: usize = at.parse().map_err(|_| "bad index".to_string())?;
                let input: Vec<String> = cases[idx].iter().map(|v| v.display()).collect();
                (
                    format!(
                        "the {side} exceeded the step budget of {budget} Python line/jump events \
                         at input ({}); a non-terminating or very long run is not judged",
                        input.join(", ")
                    ),
                    format!("the {side} exceeded the step budget of {budget} at input ({})", input.join(", ")),
                )
            };
            if !found.is_empty() {
                return Ok(finish(found, judged, exceptions, short));
            }
            return Ok(done(ReviewVerdict::Error { reason }, exceptions));
        }
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() == 2 && parts[1] == "SKIP" {
            not_judged += 1;
            continue;
        }
        if parts.len() < 4 {
            continue;
        }
        judged += 1;
        if parts[1] == "EQX" {
            exceptions += 1;
        }
        if parts[1] == "NE" {
            let idx: usize = parts[0].parse().map_err(|_| "bad index".to_string())?;
            let input: Vec<String> = cases[idx].iter().map(|v| v.display()).collect();
            found.push(Disagreement {
                input: format!("({})", input.join(", ")),
                candidate: parts[2].to_string(),
                reference: parts[3].to_string(),
            });
        }
    }
    if !found.is_empty() {
        return Ok(finish(found, judged, exceptions, String::new()));
    }
    // n is the number of cases actually judged (boundary + literal + random +
    // the large corners that finished, deduplicated), not the requested random
    // count: an honest tally. not_judged is the number of large-magnitude
    // corners where a side ran out of budget; both are in the receipt.
    Ok(done(ReviewVerdict::Equivalent { n: cases.len() as u32 - not_judged, not_judged }, exceptions))
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
        ReviewSpec { func: "f".into(), args, n: 200, seed, budget: DEFAULT_BUDGET }
    }

    fn matches_type(v: &PyVal, t: ArgType) -> bool {
        matches!(
            (v, t),
            (PyVal::Int(_), ArgType::Int)
                | (PyVal::Str(_), ArgType::Str)
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
