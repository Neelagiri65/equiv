//! `equiv review-pr`: check a pull request's functions against the base ref.
//!
//! Three rules shape this file, all the same rule underneath (a review that
//! did not happen must never read as a pass):
//!
//! 1. The base ref must resolve to a commit. If it does not, nothing can be
//!    compared, and the run ends with its own exit code (4) instead of
//!    reporting every function as "could not be checked".
//! 2. The manifest at the base ref is the authority on what is checked. The
//!    PR under review can add lines (they run too, and are named as new), but
//!    it cannot remove or narrow a line that base lists: those run as written
//!    at base, and the output says the two manifests differ.
//! 3. A function that base lists and that existed at base, but no longer
//!    exists in the PR, is reported as a behaviour change (exit 1), not as
//!    "could not check".

use crate::args::{self, Parsed};
use equiv_review::{ArgType, DetectedStatus, Domain, PrCheck, ReviewSpec};
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;

/// `review-pr` could not read the base ref: the review did not run.
pub const EXIT_BASE_UNREADABLE: u8 = 4;

/// The detector's reason for a function present at base and absent at head.
/// Pinned by tests/removed_function.rs: if the detector's wording changes,
/// that test fails rather than removals quietly turning into "not checked".
const REMOVED_REASON: &str = "removed in this change";

/// Sixteen bytes nobody else on the machine can predict. The kernel's
/// generator where there is one; otherwise the standard library's
/// per-process random hash keys.
fn random16() -> [u8; 16] {
    let mut b = [0u8; 16];
    #[cfg(unix)]
    {
        use std::io::Read as _;
        if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
            return b;
        }
    }
    use std::hash::{BuildHasher, Hasher};
    let addr = &b as *const _ as usize;
    for (i, chunk) in b.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.write_usize(addr ^ i);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    b
}

/// Where the base version of each file is written for the evaluator to load.
/// The directory is created exclusively, under a name that cannot be guessed,
/// readable by this user only, and removed when the review ends. The base
/// files used to sit at a guessable path in the shared temp directory, where
/// the code under review could replace the reference it was compared with.
struct PrivateDir {
    path: PathBuf,
    written: std::cell::Cell<u32>,
}

impl PrivateDir {
    fn create() -> Result<Self, String> {
        let mut last = String::new();
        for _ in 0..4 {
            let path = std::env::temp_dir().join(format!("equiv-{}", crate::hex(&random16())));
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            // Not recursive: creation fails if the name already exists, so a
            // directory someone else made is never adopted.
            match b.create(&path) {
                Ok(()) => return Ok(PrivateDir { path, written: std::cell::Cell::new(0) }),
                Err(e) => last = format!("{}: {e}", path.display()),
            }
        }
        Err(format!("cannot create a private directory for the base files ({last})"))
    }

    fn write(&self, name: &str, file: &str, src: &[u8]) -> Result<PathBuf, String> {
        let k = self.written.get();
        self.written.set(k + 1);
        let slug: String = format!("{name}_{file}")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(80)
            .collect();
        let p = self.path.join(format!("base_{k}_{slug}.py"));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&p).and_then(|mut f| f.write_all(src)).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(p)
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn git(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git").args(["-c", "core.quotepath=false"]).args(args).output().map_err(|e| format!("git: {e}"))
}

/// The commit the base ref names, or why it cannot be read.
fn resolve_base(base: &str) -> Result<String, String> {
    if base.is_empty() || base.starts_with('-') {
        return Err(format!("`{base}` is not a ref name"));
    }
    let out = git(&["rev-parse", "--verify", &format!("{base}^{{commit}}")])?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !sha.is_empty() {
        return Ok(sha);
    }
    let why = String::from_utf8_lossy(&out.stderr);
    Err(why.lines().last().unwrap_or("git rev-parse failed").trim().to_string())
}

/// The file's content at the (already resolved) base commit; None when the
/// path does not exist there. The commit is known to be readable, so a
/// failure here is about the path, not the ref. `spec` is git's path syntax:
/// `dir/file` from the repository root, `./dir/file` from the current directory.
fn show_at(sha: &str, spec: &str) -> Option<Vec<u8>> {
    match git(&["show", &format!("{sha}:{spec}")]) {
        Ok(o) if o.status.success() => Some(o.stdout),
        _ => None,
    }
}

/// A manifest path (the manifest itself, or a file it names) as git sees it
/// from the current directory, so the base copy and the working-tree copy are
/// always the same file. A path git cannot place in the tree is an error:
/// treating it as "not at base" would quietly drop the base manifest.
fn tree_spec(path: &str) -> Result<String, String> {
    if std::path::Path::new(path).is_absolute() || path.split(['/', '\\']).any(|p| p == "..") {
        return Err(format!("`{path}`: give a path inside the repository, relative to the current directory"));
    }
    Ok(format!("./{}", path.strip_prefix("./").unwrap_or(path)))
}

#[derive(Clone)]
enum Line {
    /// `file : fn : sig [: n [: seed]]`: head against base.
    Plain { file: String, func: String, n: u32, seed: u64 },
    /// `twin : cand : ref : fn : sig [: flags]`: two files of the PR's tree.
    Twin { cand: String, refr: String, func: String, errors_agree: bool, same_host: bool },
}

#[derive(Clone)]
struct ManifestLine {
    /// The line with its fields trimmed and rejoined: what "the same line"
    /// means when the base and head manifests are compared.
    norm: String,
    line: Line,
    args: Vec<ArgType>,
    domains: Vec<Domain>,
}

pub fn parse_sig(sig: &str) -> Result<(Vec<ArgType>, Vec<Domain>), String> {
    let parsed = sig
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| ArgType::parse_domain(s).ok_or_else(|| format!("bad arg type `{}`", s.trim())))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(parsed.into_iter().unzip())
}

/// Every line must be fully understood: a field that is ignored would let a
/// manifest claim one thing and run another.
fn parse_manifest(label: &str, text: &str) -> Result<Vec<ManifestLine>, String> {
    let mut out: Vec<ManifestLine> = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let at = |msg: String| format!("{label}:{}: {msg}", lineno + 1);
        let parts: Vec<&str> = raw.split(':').map(str::trim).collect();
        let (line, sig) = if parts[0] == "twin" {
            let usage = "expected `twin : cand.py : ref.py : fn : sig [: errors-agree same-host]`";
            if parts.len() < 5 || parts.len() > 6 {
                return Err(at(usage.to_string()));
            }
            let (mut errors_agree, mut same_host) = (false, false);
            for flag in parts.get(5).map(|f| f.split_whitespace().collect::<Vec<_>>()).unwrap_or_default() {
                match flag {
                    "errors-agree" => errors_agree = true,
                    "same-host" => same_host = true,
                    other => return Err(at(format!("unknown twin flag `{other}`; {usage}"))),
                }
            }
            (
                Line::Twin {
                    cand: parts[1].to_string(),
                    refr: parts[2].to_string(),
                    func: parts[3].to_string(),
                    errors_agree,
                    same_host,
                },
                parts[4],
            )
        } else {
            if parts.len() < 3 || parts.len() > 5 {
                return Err(at("expected `file : fn : sig [: n [: seed]]`".to_string()));
            }
            let n = match parts.get(3) {
                Some(s) => args::number::<u32>("the case count n", s).map_err(&at)?,
                None => equiv_review::DEFAULT_N,
            };
            let seed = match parts.get(4) {
                Some(s) => args::number::<u64>("the seed", s).map_err(&at)?,
                None => 1,
            };
            (Line::Plain { file: parts[0].to_string(), func: parts[1].to_string(), n, seed }, parts[2])
        };
        let (args, domains) = parse_sig(sig).map_err(&at)?;
        let norm = parts.iter().map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" : ");
        if out.iter().all(|l| l.norm != norm) {
            out.push(ManifestLine { norm, line, args, domains });
        }
    }
    Ok(out)
}

/// The names the detector reports as present at base and absent at head.
fn removed_names(base_src: &str, head_src: &str) -> HashSet<String> {
    equiv_review::detect_changed_functions(base_src, head_src)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| matches!(&d.status, DetectedStatus::NotChecked { reason } if reason == REMOVED_REASON))
        .map(|d| d.name)
        .collect()
}

/// Put `top` above the renderer's headline and `after` below it. The renderer
/// (equiv-review) owns the comment; this only adds the gate's own statements
/// to it. With `drop_headline`, the headline is removed: used when nothing
/// was run, so no sentence about checked functions is left standing.
fn compose(rendered: &str, top: &str, after: &str, drop_headline: bool) -> String {
    if top.is_empty() && after.is_empty() {
        return rendered.to_string();
    }
    let anchor = "## equiv review\n\n";
    let Some(start) = rendered.find(anchor).map(|i| i + anchor.len()) else {
        return format!("{}\n{top}{after}\n{rendered}", equiv_review::MARKER);
    };
    let end = rendered[start..].find('\n').map(|i| start + i + 1).unwrap_or(rendered.len());
    let headline = if drop_headline { "" } else { &rendered[start..end] };
    let after = if after.is_empty() { String::new() } else { format!("\n{after}") };
    format!("{}{top}{headline}{after}{}", &rendered[..start], &rendered[end..])
}

fn tick(s: &str) -> String {
    format!("`{}`", s.replace('`', ""))
}

pub fn run(argv: &[String], safety: u64, budget: u64) -> Result<u8, String> {
    let a: Parsed = args::parse(
        "review-pr",
        argv,
        &["--auto", "--no-harvest", "--same-host"],
        &["--base", "--receipts-out", "--attest-out", "--budget"],
    )?;
    let auto = a.has("--auto");
    let same_host = a.has("--same-host");
    match (auto, a.pos.len()) {
        (true, 0) | (false, 1) => {}
        (true, _) => return Err("review-pr: pass a manifest path or --auto, not both".into()),
        (false, 0) => return Err("review-pr: missing manifest path (or pass --auto)".into()),
        (false, _) => return Err(format!("review-pr: unexpected argument `{}`", a.pos[1])),
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let base = a
        .value("--base")
        .map(str::to_string)
        .or_else(|| env("EQUIV_BASE_REF"))
        .or_else(|| env("GITHUB_BASE_REF"))
        .unwrap_or_else(|| "origin/main".to_string());

    // Nothing below means anything unless base can be read. This is its own
    // exit code so a wrapper can tell "the comparison target is missing" from
    // "a function could not be checked".
    let base_sha = match resolve_base(&base) {
        Ok(sha) => sha,
        Err(why) => {
            eprintln!(
                "error: review-pr: cannot read the base ref `{base}` ({why}). Nothing was compared, so this is \
                 not a result. Fetch the base branch (in GitHub Actions: `fetch-depth: 0` on actions/checkout) \
                 or pass the right ref with --base."
            );
            return Ok(EXIT_BASE_UNREADABLE);
        }
    };

    let tmp = PrivateDir::create()?;
    let mut checks: Vec<PrCheck> = Vec::new();
    // Functions that changed but could not be checked. Surfaced in the
    // comment so a green result never hides them (closes issue #1).
    let mut unchecked: Vec<(String, String)> = Vec::new();
    // (function, file): listed at base, existed at base, gone in this PR.
    let mut removed: Vec<(String, String)> = Vec::new();
    let mut manifest_note = String::new();

    if auto {
        // Detect changed functions from the diff. No manifest needed, so
        // a refactored function can no longer be silently left out.
        //
        // The diff is taken against the merge-base of the base ref and
        // HEAD when git can find one: a local run from a branch that is
        // behind its base then reviews only the branch's own changes,
        // never a function that moved on the base branch after the
        // branch point (which would otherwise appear with the roles
        // reversed). In the Action's merge-ref checkout the merge-base
        // is the base tip itself, so nothing changes there. No
        // merge-base (unrelated histories, shallow clone) falls back to
        // the base commit itself.
        let diff_base = git(&["merge-base", &base_sha, "HEAD"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| base_sha.clone());
        // --name-status with rename detection: a renamed file is the
        // same functions under a new path, so its base version is read
        // from the old path and its changed functions are checked
        // instead of being listed as "new" (plus "removed").
        let listed = git(&["diff", "-M", "--name-status", &diff_base, "--", "*.py"])?;
        if !listed.status.success() {
            return Err("git diff failed (need a git repo and a valid base ref)".into());
        }
        let mut entries: Vec<(String, String)> = Vec::new(); // (head path, base path)
        for line in String::from_utf8_lossy(&listed.stdout).lines() {
            let f: Vec<&str> = line.split('\t').map(str::trim).collect();
            match f.as_slice() {
                [st, old, new] if st.starts_with('R') || st.starts_with('C') => {
                    entries.push((new.to_string(), old.to_string()));
                }
                [_, path] if !path.is_empty() => entries.push((path.to_string(), path.to_string())),
                _ => {}
            }
        }
        for (file, base_file) in &entries {
            let head_src = std::fs::read_to_string(file).unwrap_or_default();
            let base_src = show_at(&diff_base, base_file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            for d in equiv_review::detect_changed_functions(&base_src, &head_src)? {
                match d.status {
                    DetectedStatus::Checkable { args } => {
                        let base_path = tmp.write(&d.name, file, base_src.as_bytes())?;
                        checks.push(PrCheck {
                            name: d.name.clone(),
                            head_path: PathBuf::from(file),
                            base_path,
                            spec: ReviewSpec {
                                func: d.name,
                                args,
                                n: equiv_review::DEFAULT_N,
                                seed: 1,
                                budget,
                                errors_agree: false,
                                domains: vec![],
                                same_host,
                            },
                            twin: None,
                        });
                    }
                    DetectedStatus::NotChecked { reason } => unchecked.push((d.name, reason)),
                }
            }
        }
    } else {
        let manifest = a.pos[0].as_str();
        let head_text = match std::fs::read_to_string(manifest) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{manifest}: {e}")),
        };
        let base_text = show_at(&base_sha, &tree_spec(manifest)?).map(|b| String::from_utf8_lossy(&b).into_owned());
        let head_lines = match &head_text {
            Some(t) => parse_manifest(manifest, t).map_err(|e| format!("{e} (the manifest in this PR)"))?,
            None => Vec::new(),
        };
        // (line, listed at base?)
        let mut lines: Vec<(ManifestLine, bool)> = Vec::new();
        match &base_text {
            None if head_text.is_none() => {
                return Err(format!("{manifest}: no such manifest in this PR or at base `{base}`"));
            }
            None => {
                manifest_note = format!(
                    "Manifest: {} is new in this PR (it does not exist at base {}), so every line checked \
                     here was written by this PR.\n",
                    tick(manifest),
                    tick(&base)
                );
                lines.extend(head_lines.iter().cloned().map(|l| (l, false)));
            }
            Some(bt) => {
                let base_lines = parse_manifest(manifest, bt).map_err(|e| format!("{e} (the manifest at base `{base}`)"))?;
                let at_base: HashSet<&str> = base_lines.iter().map(|l| l.norm.as_str()).collect();
                let at_head: HashSet<&str> = head_lines.iter().map(|l| l.norm.as_str()).collect();
                let dropped: Vec<&ManifestLine> = base_lines.iter().filter(|l| !at_head.contains(l.norm.as_str())).collect();
                let added: Vec<&ManifestLine> = head_lines.iter().filter(|l| !at_base.contains(l.norm.as_str())).collect();
                if !dropped.is_empty() || !added.is_empty() || head_text.is_none() {
                    let what = if head_text.is_none() {
                        format!("{} was deleted in this PR", tick(manifest))
                    } else {
                        format!("{} in this PR differs from base {}", tick(manifest), tick(&base))
                    };
                    manifest_note = format!(
                        "Manifest: {what}. The base version decides what is checked: every line listed at \
                         base was run as written at base.\n"
                    );
                    if !dropped.is_empty() {
                        manifest_note.push_str("\nListed at base, removed or edited in this PR (still run as written at base):\n");
                        for l in &dropped {
                            manifest_note.push_str(&format!("- {}\n", tick(&l.norm)));
                        }
                    }
                    if !added.is_empty() {
                        manifest_note.push_str(&format!(
                            "\nNew in this PR (also run; the last {} of the table below):\n",
                            match added.len() {
                                1 => "row".to_string(),
                                n => format!("{n} rows"),
                            }
                        ));
                        for l in &added {
                            manifest_note.push_str(&format!("- {}\n", tick(&l.norm)));
                        }
                    }
                }
                lines.extend(base_lines.iter().cloned().map(|l| (l, true)));
                lines.extend(added.into_iter().cloned().map(|l| (l, false)));
            }
        }

        // Which names were removed, per file (one detector run per file).
        let mut gone: HashMap<String, HashSet<String>> = HashMap::new();
        let mut is_removed = |path: &str, func: &str, base_src: &[u8]| -> bool {
            gone.entry(path.to_string())
                .or_insert_with(|| {
                    let head_src = std::fs::read_to_string(path).unwrap_or_default();
                    removed_names(&String::from_utf8_lossy(base_src), &head_src)
                })
                .contains(func)
        };
        for (l, listed_at_base) in lines {
            match l.line {
                Line::Twin { cand, refr, func, errors_agree, same_host: line_same_host } => {
                    // Both files come from the PR's tree. A function that a
                    // base line names and that was in one of them at base,
                    // but is gone now, is a removal like any other.
                    let mut any_gone = false;
                    if listed_at_base {
                        for path in [&cand, &refr] {
                            if let Some(b) = show_at(&base_sha, &tree_spec(path)?) {
                                if is_removed(path, &func, &b) {
                                    removed.push((func.clone(), path.clone()));
                                    any_gone = true;
                                }
                            }
                        }
                    }
                    if any_gone {
                        continue;
                    }
                    checks.push(PrCheck {
                        name: func.clone(),
                        head_path: PathBuf::from(&cand),
                        base_path: PathBuf::from(&refr),
                        spec: ReviewSpec {
                            func,
                            args: l.args,
                            n: equiv_review::DEFAULT_N,
                            seed: 1,
                            budget,
                            errors_agree,
                            domains: l.domains,
                            same_host: same_host || line_same_host,
                        },
                        twin: Some((cand, refr)),
                    });
                }
                Line::Plain { file, func, n, seed } => {
                    // None: the file did not exist at base (the base commit
                    // itself is readable). The check then runs against an
                    // empty base and reports that there is nothing to
                    // compare with, as before.
                    let base_bytes = show_at(&base_sha, &tree_spec(&file)?);
                    if let Some(b) = &base_bytes {
                        if listed_at_base && is_removed(&file, &func, b) {
                            removed.push((func, file));
                            continue;
                        }
                    }
                    let base_path = tmp.write(&func, &file, &base_bytes.unwrap_or_default())?;
                    checks.push(PrCheck {
                        name: func.clone(),
                        head_path: PathBuf::from(&file),
                        base_path,
                        spec: ReviewSpec { func, args: l.args, n, seed, budget, errors_agree: false, domains: l.domains, same_host },
                        twin: None,
                    });
                }
            }
        }
    }

    if checks.is_empty() && unchecked.is_empty() && removed.is_empty() {
        if auto {
            eprintln!("review-pr: nothing changed to review");
        } else {
            eprintln!("review-pr: the manifest lists no functions; nothing to review");
        }
        return Ok(0);
    }
    // Inputs harvested from the repository's own calls and tests
    // (0.17.0): one pass over every tracked Python file, at head.
    // `--no-harvest` turns it off.
    let harvested: Vec<Vec<equiv_review::Slots>> = if a.has("--no-harvest") {
        Vec::new()
    } else {
        let wanted: Vec<(String, PathBuf, Vec<ArgType>)> =
            checks.iter().map(|c| (c.spec.func.clone(), c.head_path.clone(), c.spec.args.clone())).collect();
        let files = equiv_review::harvest::python_files(std::path::Path::new("."));
        equiv_review::harvest::harvest(&wanted, &files)
    };
    let (items, code) = equiv_review::run_pr_with_inputs(&checks, &harvested, safety);
    // A changed function that auto-detection could not check is "not
    // checked" like a safety stop or a refusal: nothing diverged, but
    // the run is not a clean pass (0). Exit 2 says so; whether that
    // blocks the merge is the Action's `fail-on-not-checked` input.
    let code = if code == 0 && !unchecked.is_empty() { 2 } else { code };
    // A listed function that is gone is a behaviour change: every caller of
    // it now fails. Same status as a divergence.
    let code = if removed.is_empty() { code } else { 1 };

    // Local-first verdict-rate metrics (fail-silent, counts only, no
    // network). This build reviews Python only, so the tag is "python".
    crate::telemetry::record(&items, &unchecked, "python");

    // Sign if a key is configured (CI secret). Absent key => unsigned,
    // never an error: fork PRs legitimately have no secret access.
    let key = match std::env::var("EQUIV_SIGNING_KEY") {
        Ok(s) if !s.trim().is_empty() => {
            Some(equiv_core::SigningKey::from_hex(&s).map_err(|e| format!("bad EQUIV_SIGNING_KEY: {e:?}"))?)
        }
        _ => None,
    };
    let signer_hex = key.as_ref().map(|k| crate::hex(&k.public_key()));

    if let Some(k) = &key {
        // Durable, signed artifact (one TSV line per function): the
        // receipt belongs in CI artifact storage, not just the comment.
        let mut buf = String::new();
        // A safety stop has no receipt, so nothing is signed for it.
        for (it, r) in items.iter().filter_map(|it| it.outcome.as_ref().ok().map(|r| (it, r))) {
            let signed = equiv_core::SignedReceipt::sign(r.to_bytes(), k);
            buf.push_str(&format!("{}\t{}\t{}\n", it.name, crate::hex(&signed.receipt_id()), crate::hex(&signed.to_bytes())));
        }
        if let Some(p) = a.value("--receipts-out") {
            std::fs::write(p, &buf).map_err(|e| format!("{p}: {e}"))?;
        }
    }

    // in-toto attestations (DR-3): one Statement per function, written
    // for the keyless Sigstore path (cosign signs them in CI).
    if let Some(dir) = a.value("--attest-out") {
        std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
        // Base lines come first. When a PR edits a manifest line, the same
        // function is checked twice; the statement kept is the base line's.
        let mut written: HashSet<PathBuf> = HashSet::new();
        for (check, item) in checks.iter().zip(items.iter()) {
            // No receipt (safety stop) => nothing to attest.
            let Ok(receipt) = &item.outcome else { continue };
            let file = check.head_path.display().to_string();
            let stmt = equiv_review::intoto_statement(&file, &item.name, receipt);
            let path = std::path::Path::new(dir).join(format!("{}.intoto.json", item.name));
            if written.insert(path.clone()) {
                std::fs::write(&path, stmt).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }

    let mut top = String::new();
    if !removed.is_empty() {
        top.push_str(&format!(
            "**Behaviour changed: {} no longer {} in this PR.** A removed or renamed function blocks like a \
             divergence: its callers no longer get the base behaviour.\n\n| function | result | detail |\n|---|---|---|\n",
            match removed.len() {
                1 => "1 listed function".to_string(),
                n => format!("{n} listed functions"),
            },
            if removed.len() == 1 { "exists" } else { "exist" },
        ));
        for (func, file) in &removed {
            top.push_str(&format!(
                "| {} | REMOVED | {} no longer exists in {}; it was removed or renamed |\n",
                tick(func),
                tick(func),
                tick(file)
            ));
        }
        top.push('\n');
    }
    let rendered = equiv_review::render_markdown_with_unchecked(&items, signer_hex.as_deref(), &unchecked);
    print!("{}", compose(&rendered, &top, &manifest_note, items.is_empty() && unchecked.is_empty()));
    Ok(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_keeps_the_marker_first_and_places_the_notes_around_the_headline() {
        let r = format!("{}\n## equiv review\n\n1 checked function: fine.\n\n| a |\n", equiv_review::MARKER);
        let out = compose(&r, "TOP\n\n", "NOTE\n", false);
        assert!(out.starts_with(equiv_review::MARKER));
        assert!(out.contains("## equiv review\n\nTOP\n\n1 checked function: fine.\n\nNOTE\n\n| a |\n"), "{out}");
        let out = compose(&r, "TOP\n\n", "", true);
        assert!(!out.contains("checked function"), "{out}");
        assert_eq!(compose(&r, "", "", false), r);
    }

    #[test]
    fn manifest_lines_are_compared_after_trimming() {
        let a = parse_manifest("m", "m.py:f:int\n").unwrap();
        let b = parse_manifest("m", "  m.py :  f : int  \n# c\n\nm.py : f : int\n").unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].norm, b[0].norm);
        let c = parse_manifest("m", "m.py : f : int[5..5]\n").unwrap();
        assert_ne!(a[0].norm, c[0].norm);
    }

    #[test]
    fn the_private_directory_is_ours_alone_and_goes_away() {
        let d = PrivateDir::create().unwrap();
        let p = d.write("f", "a/b.py", b"x = 1\n").unwrap();
        assert!(p.starts_with(&d.path) && p.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&d.path).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let other = PrivateDir::create().unwrap();
        assert_ne!(d.path, other.path);
        let path = d.path.clone();
        drop(d);
        assert!(!path.exists());
    }
}
