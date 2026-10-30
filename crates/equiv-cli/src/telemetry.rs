//! Local-first verdict-rate metrics (telemetry slice 1).
//!
//! Goal: measure the *verdict rate* (`checked / changed_functions`) and the
//! reason breakdown for the functions equiv could NOT check, on real PRs.
//! This is the tournament's STEP-0 gating number.
//!
//! Trust posture (non-negotiable, mirrors the receipt discipline):
//! - LOCAL ONLY. This slice never touches the network. It appends one JSON
//!   line per run to `equiv-metrics.jsonl` in the working directory.
//! - Counts only. NEVER function names, source, paths, arg values, outputs,
//!   receipt-ids, repo URL/name, author, branch, or commit SHA. Only counts,
//!   a language tag, the equiv version, a day-granular stamp, and a
//!   pseudonymous per-repo id.
//! - Fail-silent. A metrics write must NEVER fail or slow a review. Any error
//!   is swallowed (one stderr note at most). Determinism is the product.
//! - Opt-out honoured: `EQUIV_METRICS=off` or `DO_NOT_TRACK=1` disables it.
//!
//! The network upload is a later, explicitly opt-in slice; the payload built
//! here is the same shape it will send.

use equiv_review::{ReviewItem, ReviewVerdict};

/// Stable category keys for why a changed function was not checked. The human
/// reason text may change; these keys must not (they key the histogram).
fn classify_reason(reason: &str) -> &'static str {
    let r = reason.to_ascii_lowercase();
    if r.contains("could not be parsed") {
        "base_unparseable"
    } else if r.contains("new function") {
        "new_function"
    } else if r.contains("removed") {
        "removed"
    } else if r.contains("async") {
        "async_function"
    } else if r.contains("method") {
        "method"
    } else if r.contains("nested") {
        "nested_function"
    } else if r.contains("signature") {
        "signature_not_simple_positional"
    } else if r.contains("type hint") {
        "no_supported_type_hint"
    } else {
        "other"
    }
}

/// The eight known keys plus "other", in a fixed order so the JSON object is
/// deterministic across runs.
const REASON_KEYS: [&str; 9] = [
    "no_supported_type_hint",
    "signature_not_simple_positional",
    "async_function",
    "method",
    "new_function",
    "nested_function",
    "removed",
    "base_unparseable",
    "other",
];

/// Non-cryptographic, stable, pseudonymous repo id (FNV-1a of the remote URL).
/// Enough to count distinct repos in a local file the operator owns; the
/// network slice will switch to a salted SHA-256 before anything leaves the
/// machine. Returns "local" when there is no remote.
fn repo_id() -> String {
    let url = std::process::Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if url.is_empty() {
        return "local".to_string();
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Days since the Unix epoch (day-granular; no sub-day timestamp is logged).
fn day_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

/// "ci" under GitHub Actions, else "cli". (pre-commit detection is a later slice.)
fn run_context() -> &'static str {
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        "ci"
    } else {
        "cli"
    }
}

/// Build the one-line JSON metrics event. Pure and deterministic given its
/// inputs (repo_id/day/context are injected), so it is unit-testable. Every
/// field is an integer or a controlled-vocabulary string, so no JSON string
/// escaping is required.
fn build_event(
    items: &[ReviewItem],
    unchecked: &[(String, String)],
    language: &str,
    run_context: &str,
    repo_id: &str,
    day_epoch: u64,
) -> String {
    let (mut equivalent, mut counterexample, mut error, mut refused) = (0u32, 0u32, 0u32, 0u32);
    for it in items {
        match it.receipt.verdict {
            ReviewVerdict::Equivalent { .. } => equivalent += 1,
            ReviewVerdict::Counterexample { .. } => counterexample += 1,
            ReviewVerdict::Error { .. } => error += 1,
            ReviewVerdict::Refused { .. } => refused += 1,
        }
    }

    let mut reasons: std::collections::BTreeMap<&'static str, u32> = std::collections::BTreeMap::new();
    for (_name, reason) in unchecked {
        *reasons.entry(classify_reason(reason)).or_insert(0) += 1;
    }

    let checked = items.len();
    let not_checked = unchecked.len();
    let changed = checked + not_checked;

    let reasons_json = REASON_KEYS
        .iter()
        .map(|k| format!("\"{k}\":{}", reasons.get(k).copied().unwrap_or(0)))
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"schema\":1,\"equiv_version\":\"{ver}\",\"day_epoch\":{day},\
         \"language\":\"{lang}\",\"run_context\":\"{ctx}\",\"repo_id\":\"{repo}\",\
         \"changed_functions\":{changed},\"checked\":{checked},\"not_checked\":{not_checked},\
         \"verdicts\":{{\"equivalent\":{equivalent},\"counterexample\":{counterexample},\
         \"error\":{error},\"refused\":{refused}}},\
         \"not_checked_reasons\":{{{reasons_json}}}}}",
        ver = env!("CARGO_PKG_VERSION"),
        day = day_epoch,
        lang = language,
        ctx = run_context,
        repo = repo_id,
    )
}

/// Append one metrics line to `equiv-metrics.jsonl` (or `$EQUIV_METRICS_OUT`).
/// Best-effort and fail-silent: disabled by `EQUIV_METRICS=off` / `DO_NOT_TRACK`,
/// and any I/O error is swallowed after a single stderr note.
pub fn record(items: &[ReviewItem], unchecked: &[(String, String)], language: &str) {
    let off = std::env::var("EQUIV_METRICS").as_deref() == Ok("off")
        || std::env::var("DO_NOT_TRACK").is_ok_and(|v| !v.is_empty() && v != "0");
    if off {
        return;
    }
    let line = build_event(
        items,
        unchecked,
        language,
        run_context(),
        &repo_id(),
        day_epoch(),
    );
    let path = std::env::var("EQUIV_METRICS_OUT").unwrap_or_else(|_| "equiv-metrics.jsonl".to_string());
    use std::io::Write;
    let write = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if write.is_err() {
        eprintln!("equiv: note: could not write {path} (metrics skipped)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use equiv_review::ReviewReceipt;

    fn item(name: &str, verdict: ReviewVerdict) -> ReviewItem {
        ReviewItem {
            name: name.to_string(),
            receipt: ReviewReceipt {
                candidate_sha256: [0; 32],
                reference_sha256: [0; 32],
                spec_sha256: [0; 32],
                verdict,
                checker_version: "test".to_string(),
            },
        }
    }

    #[test]
    fn classifies_all_known_reasons() {
        assert_eq!(classify_reason("no supported type hint on every parameter"), "no_supported_type_hint");
        assert_eq!(classify_reason("async function, not checked in this version"), "async_function");
        assert_eq!(classify_reason("method, not checked (module-level functions only)"), "method");
        assert_eq!(classify_reason("nested function, not checked"), "nested_function");
        assert_eq!(classify_reason("signature is not a simple positional match"), "signature_not_simple_positional");
        assert_eq!(classify_reason("new function, no base version to compare against"), "new_function");
        assert_eq!(classify_reason("removed in this change"), "removed");
        assert_eq!(classify_reason("base version could not be parsed; cannot diff"), "base_unparseable");
        assert_eq!(classify_reason("something we never emit"), "other");
    }

    #[test]
    fn event_json_is_exact_and_deterministic() {
        let items = vec![
            item("a", ReviewVerdict::Equivalent { n: 300 }),
            item("b", ReviewVerdict::Counterexample {
                input: "(1)".into(),
                candidate: "1".into(),
                reference: "2".into(),
            }),
            item("c", ReviewVerdict::Error { reason: "boom".into() }),
        ];
        let unchecked = vec![
            ("d".to_string(), "no supported type hint on every parameter".to_string()),
            ("e".to_string(), "async function, not checked in this version".to_string()),
            ("f".to_string(), "no supported type hint on every parameter".to_string()),
        ];
        let json = build_event(&items, &unchecked, "python", "cli", "local", 20649);
        let expected = format!(
            "{{\"schema\":1,\"equiv_version\":\"{}\",\"day_epoch\":20649,\
             \"language\":\"python\",\"run_context\":\"cli\",\"repo_id\":\"local\",\
             \"changed_functions\":6,\"checked\":3,\"not_checked\":3,\
             \"verdicts\":{{\"equivalent\":1,\"counterexample\":1,\"error\":1,\"refused\":0}},\
             \"not_checked_reasons\":{{\"no_supported_type_hint\":2,\
             \"signature_not_simple_positional\":0,\"async_function\":1,\"method\":0,\
             \"new_function\":0,\"nested_function\":0,\"removed\":0,\"base_unparseable\":0,\
             \"other\":0}}}}",
            env!("CARGO_PKG_VERSION"),
        );
        assert_eq!(json, expected);
    }
}
