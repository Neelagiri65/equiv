//! `equiv hook claude-code`: the PostToolUse hook. After the agent edits or
//! writes a Python file that git tracks, the functions that changed since the
//! last commit are checked exactly as `review-pr --auto` checks them (same
//! detector, same checks, same verdict code), and the agent is told about
//! every one of them: a difference with its input, the same behaviour with
//! its receipt, not checked with the reason, or could not check.
//!
//! Advisory only: it never blocks, and it exits 0 for every outcome. It is
//! silent only when there is nothing to compare (not a .py file, not in a git
//! repository, no version at the last commit, or no function changed);
//! whenever something changed and could not be checked, it says so.
//!
//! One check per edit. Claude Code runs every registered copy of a hook, and a
//! project can have two (one from `equiv init claude-code`, one from the
//! plugin). Both get the same event, so the first copy to take the tool call
//! reports and any other copy of that tool call stays silent.
//!
//! Inputs harvested from the project's other files are not used here, so a
//! hook run costs the same whatever the size of the repository; `equiv
//! review-pr --auto` adds them.

use crate::pr::{auto_check, git_in, PrivateDir};
use equiv_review::{DetectedStatus, ReviewItem, ReviewVerdict};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// At most this many functions are checked per edit; the rest are named.
const MAX_CHECKS: usize = 6;
/// The per-function wall-clock safety limit in the hook, in seconds (lower
/// than the CLI default: the agent is waiting). $EQUIV_SAFETY_SECONDS can
/// only lower it.
const SAFETY_CAP_SECS: u64 = 30;
const FULL_RUN: &str = "`equiv review-pr --auto --base HEAD`";

pub fn run(args: &[String]) -> Result<u8, String> {
    match args {
        [agent] if agent == "claude-code" => {}
        [] => return Err("hook: name the agent; supported: claude-code".into()),
        [agent] => return Err(format!("hook: unknown agent `{agent}`; supported: claude-code")),
        [_, extra, ..] => return Err(format!("hook: unexpected argument `{extra}`")),
    }
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return Ok(0);
    }
    if let Some(context) = after_edit(&input) {
        let out = json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": context}});
        let _ = writeln!(std::io::stdout(), "{out}");
    }
    Ok(0)
}

/// What to tell the agent after this tool call, or None to stay silent.
fn after_edit(input: &[u8]) -> Option<String> {
    let event: Value = serde_json::from_slice(input).ok()?;
    if event.get("hook_event_name").is_some_and(|e| e.as_str() != Some("PostToolUse")) {
        return None;
    }
    let file = event.get("tool_input")?.get("file_path")?.as_str()?;
    if !file.ends_with(".py") {
        return None;
    }
    let path = match Path::new(file) {
        p if p.is_absolute() => p.to_path_buf(),
        p => event.get("cwd").and_then(Value::as_str).map(PathBuf::from).or_else(|| std::env::current_dir().ok())?.join(p),
    };
    let (dir, name) = (path.parent()?, path.file_name()?.to_str()?);
    // Inside a git repository, with a version at the last commit. `./name`
    // is read relative to `dir`, so no path prefix has to match (macOS temp
    // paths reach git through a symlink).
    let prefix = git_in(dir, &["rev-parse", "--show-prefix"]).ok().filter(|o| o.status.success())?;
    let committed = git_in(dir, &["show", &format!("HEAD:./{name}")]).ok().filter(|o| o.status.success())?.stdout;
    let now = std::fs::read(&path).ok()?;
    if now == committed {
        return None;
    }
    if another_copy_took(&event) {
        return None;
    }
    let rel = format!("{}{name}", String::from_utf8_lossy(&prefix.stdout).trim_end_matches(['\n', '\r']));
    report(&path, &rel, &committed, &now)
}

/// Tool calls taken longer ago than this are forgotten.
const CLAIM_SECS: u64 = 3600;

/// Whether another copy of the hook has already taken this tool call. The
/// first copy to create a file named after the session and the tool call, in a
/// directory only this user can write, takes it. An event without a tool-call
/// id, or any problem with that directory, means "not taken": a check is never
/// lost to bookkeeping, at worst an edit is reported twice as before.
fn another_copy_took(event: &Value) -> bool {
    let field = |k: &str| event.get(k).and_then(Value::as_str).unwrap_or("");
    let (session, call) = (field("session_id"), field("tool_use_id"));
    if call.is_empty() {
        return false;
    }
    let Some(dir) = claims_dir() else { return false };
    forget_old_claims(&dir);
    let safe = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(120).collect::<String>();
    // The length keeps ("a_b", "c") and ("a", "b_c") apart.
    let claim = dir.join(format!("{}.{}.{}", session.len(), safe(session), safe(call)));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    match o.open(&claim) {
        Ok(_file) => {
            // A directory someone else owns decides nothing: files in it could
            // have been put there to keep this hook quiet.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let owner = |p: &Path| std::fs::symlink_metadata(p).map(|m| m.uid()).ok();
                if owner(&dir).is_none() || owner(&dir) != _file.metadata().map(|m| m.uid()).ok() {
                    let _ = std::fs::remove_file(&claim);
                }
            }
            false
        }
        Err(e) => e.kind() == std::io::ErrorKind::AlreadyExists && ours(&dir, &claim),
    }
}

/// Whether an existing claim can be believed: on Unix, only if this user owns
/// both the directory and the claim.
fn ours(dir: &Path, claim: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = |p: &Path| std::fs::symlink_metadata(p).map(|m| m.uid()).ok();
        let Ok(probe) = PrivateDir::create() else { return false };
        let me = owner(probe.dir());
        me.is_some() && owner(dir) == me && owner(claim) == me
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, claim);
        true
    }
}

/// The directory of taken tool calls: made for this user only, and used only
/// if it is a real directory that nobody else can write. Ownership is checked
/// where a claim is believed (`ours`).
fn claims_dir() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("equiv-hook-claims");
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o022 != 0 {
            return None;
        }
    }
    Some(dir)
}

fn forget_old_claims(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > CLAIM_SECS);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Check the changed functions of one file and say what happened to each.
fn report(path: &Path, rel: &str, committed: &[u8], now: &[u8]) -> Option<String> {
    let cannot = |why: String| {
        Some(format!(
            "equiv could not check the functions changed in {rel} since the last commit: {why}. Nothing was \
             compared; {FULL_RUN} shows the same once the cause is fixed."
        ))
    };
    let budget = match crate::step_budget(None) {
        Ok(b) => b,
        Err(e) => return cannot(e),
    };
    let safety = match crate::safety_seconds() {
        Ok(s) => s.min(SAFETY_CAP_SECS),
        Err(e) => return cannot(e),
    };
    let detected =
        match equiv_review::detect_changed_functions(&String::from_utf8_lossy(committed), &String::from_utf8_lossy(now)) {
            Ok(d) => d,
            Err(e) => return cannot(e),
        };
    if detected.is_empty() {
        return None;
    }
    let tmp = match PrivateDir::create() {
        Ok(t) => t,
        Err(e) => return cannot(e),
    };

    enum Row {
        Checked(usize),
        Said(String),
    }
    let mut checks = Vec::new();
    let mut rows = Vec::new();
    for d in detected {
        rows.push(match d.status {
            DetectedStatus::Checkable { args } if checks.len() < MAX_CHECKS => match tmp.write(&d.name, rel, committed) {
                Ok(base_path) => {
                    checks.push(auto_check(d.name, path.to_path_buf(), base_path, args, budget, false));
                    Row::Checked(checks.len() - 1)
                }
                Err(e) => Row::Said(format!("`{}`: could not check: {e}", d.name)),
            },
            DetectedStatus::Checkable { .. } => {
                Row::Said(format!("`{}`: not checked in this hook run; run {FULL_RUN}", d.name))
            }
            DetectedStatus::NotChecked { reason } => Row::Said(format!("`{}`: not checked: {reason}", d.name)),
        });
    }
    let (items, _) = equiv_review::run_pr_limited(&checks, safety);
    let mut lines = vec![format!(
        "equiv compared the functions changed in {rel} with the last commit (generated inputs only; behaviour, not intent):"
    )];
    for row in rows {
        lines.push(match row {
            Row::Checked(i) => item_line(&items[i]),
            Row::Said(s) => s,
        });
    }
    Some(lines.join("\n"))
}

fn item_line(it: &ReviewItem) -> String {
    let name = &it.name;
    let r = match &it.outcome {
        Ok(r) => r,
        // "could not check: exceeded N s safety limit ...": no receipt exists.
        Err(stop) => return format!("`{name}`: {}", stop.reason()),
    };
    match &r.verdict {
        ReviewVerdict::Equivalent { n, not_judged } => {
            let id = crate::hex(&r.sha256());
            let mut s = format!("`{name}`: same behaviour on {n} generated inputs (receipt {})", &id[..12]);
            if let Some(t) = equiv_review::thin_note(r) {
                s.push_str(&format!("; {t}"));
            }
            s.push_str(&equiv_review::not_judged_note(*not_judged));
            s
        }
        ReviewVerdict::Counterexample { input, candidate, reference, .. } => format!(
            "`{name}`: differs at input {input}: now returns {candidate}, the committed version returns {reference}. \
             If this change is intended, say so; if it was meant to be a refactor, fix it."
        ),
        ReviewVerdict::Error { reason } | ReviewVerdict::Refused { reason } => {
            format!("`{name}`: could not check: {reason}")
        }
    }
}
