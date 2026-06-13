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
//! reprs are stable and identical across hosts. Floats, objects and any value
//! with nondeterministic ordering are out of this slice on purpose.

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
}

impl ArgType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "int" => ArgType::Int,
            "str" => ArgType::Str,
            "list[int]" | "list" => ArgType::ListInt,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ReviewSpec {
    pub func: String,
    pub args: Vec<ArgType>,
    pub n: u32,
    pub seed: u64,
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
            });
        }
        v.extend_from_slice(&self.n.to_le_bytes());
        v.extend_from_slice(&self.seed.to_le_bytes());
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
}

impl PyVal {
    /// A Python literal: what we hand the evaluator.
    fn to_py(&self) -> String {
        match self {
            PyVal::Int(n) => n.to_string(),
            PyVal::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
            PyVal::ListInt(xs) => {
                let inner: Vec<String> = xs.iter().map(|n| n.to_string()).collect();
                format!("[{}]", inner.join(", "))
            }
        }
    }
    /// Human readable and stable: what a counterexample shows the reviewer.
    pub fn display(&self) -> String {
        self.to_py()
    }
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
    }
}

fn gen_cases(spec: &ReviewSpec) -> Vec<Vec<PyVal>> {
    let mut rng = Rng(spec.seed | 1);
    (0..spec.n)
        .map(|_| spec.args.iter().map(|t| gen_arg(&mut rng, *t)).collect())
        .collect()
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
}

impl ReviewVerdict {
    pub fn exit_code(&self) -> i32 {
        match self {
            ReviewVerdict::Equivalent { .. } => 0,
            ReviewVerdict::Counterexample { .. } => 1,
            ReviewVerdict::Error { .. } => 2,
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
    // per-case status line; Rust interprets and builds the verdict.
    format!(
        r#"import importlib.util as u
def load(p, n):
    s = u.spec_from_file_location(n, p); m = u.module_from_spec(s); s.loader.exec_module(m); return m
cand = load({cand:?}, "cand"); ref = load({refr:?}, "ref")
cf = getattr(cand, {fn:?}); rf = getattr(ref, {fn:?})
cases = [{cases}]
for i, args in enumerate(cases):
    try: cv = repr(cf(*args)); ce = None
    except Exception as e: cv = None; ce = type(e).__name__
    try: rv = repr(rf(*args)); re_ = None
    except Exception as e: rv = None; re_ = type(e).__name__
    if ce is not None or re_ is not None:
        ok = (ce == re_)
        print(f"{{i}}\t{{'EQ' if ok else 'NE'}}\t{{ce}}\t{{re_}}")
    else:
        print(f"{{i}}\t{{'EQ' if cv == rv else 'NE'}}\t{{cv}}\t{{rv}}")
"#,
        cand = cand,
        refr = refr,
        fn = spec.func,
        cases = cases_py.join(", "),
    )
}

/// Run the oracle. `candidate_path`/`reference_path` each define `spec.func`.
pub fn review(candidate_path: &Path, reference_path: &Path, spec: &ReviewSpec) -> ReviewReceipt {
    let cand_src = std::fs::read(candidate_path).unwrap_or_default();
    let ref_src = std::fs::read(reference_path).unwrap_or_default();
    let cases = gen_cases(spec);

    let verdict = run_python(candidate_path, reference_path, spec, &cases)
        .unwrap_or_else(|reason| ReviewVerdict::Error { reason });

    ReviewReceipt {
        candidate_sha256: sha256(&cand_src),
        reference_sha256: sha256(&ref_src),
        spec_sha256: sha256(&spec.canonical()),
        verdict,
        checker_version: env!("CARGO_PKG_VERSION").into(),
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
            ReviewVerdict::Error { .. } => any_err = true,
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
            ReviewVerdict::Error { .. } => errored += 1,
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
         intent, architecture, or security. A passing result means behaviour was preserved \
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
    for line in stdout.lines() {
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
    Ok(ReviewVerdict::Equivalent { n: spec.n })
}
