//! equiv CLI (v0 skeleton). One binary, every channel (DR-1): `check` is the
//! CLI/hook surface; `fmt`/`hash`/`lint` are the format toolchain. Exit codes
//! are frozen per DR-7: 0 pass, 1 counterexample, 2 unknown, 3 invalid input.

use std::process::ExitCode;

mod telemetry;

const EXIT_INVALID: u8 = 3;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| "bad hex".to_string()))
        .collect()
}

/// Per-function wall-clock safety limit (backlog 0.3b). Read here, never in the
/// library, and never written into a receipt.
/// Step budget per call: `--budget N`, else $EQUIV_BUDGET (the Action's
/// `budget` input), else the default. Bound into the receipt either way.
fn step_budget(args: &[String]) -> Result<u64, String> {
    let raw = match args.iter().position(|a| a == "--budget") {
        Some(i) => Some(args.get(i + 1).cloned().ok_or("--budget needs a whole number of Python line/jump events")?),
        None => std::env::var("EQUIV_BUDGET").ok().filter(|v| !v.trim().is_empty()),
    };
    match raw {
        None => Ok(equiv_review::DEFAULT_BUDGET),
        Some(v) => match v.trim().parse::<u64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!("--budget (or EQUIV_BUDGET) must be a whole number of steps above 0, got `{v}`")),
        },
    }
}

fn safety_seconds() -> Result<u64, String> {
    match std::env::var("EQUIV_SAFETY_SECONDS") {
        Err(_) => Ok(equiv_review::DEFAULT_SAFETY_SECONDS),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!("EQUIV_SAFETY_SECONDS must be a whole number of seconds above 0, got `{v}`")),
        },
    }
}

/// The evaluator runs in its own process group (so the safety stop can kill
/// all of it), which puts it out of reach of the terminal's Ctrl-C. Forward
/// SIGINT/SIGTERM/SIGHUP: kill live evaluators, then die by the same signal.
#[cfg(unix)]
fn forward_interrupts() {
    extern "C" fn on_signal(sig: libc::c_int) {
        equiv_review::kill_live_evaluators();
        // SAFETY: signal/raise are async-signal-safe.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: installing a handler that only makes async-signal-safe calls.
        unsafe { libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

fn load_contract(path: &str) -> Result<equiv_core::Contract, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if path.ends_with(".eqct") {
        let src = String::from_utf8(bytes).map_err(|_| format!("{path}: not UTF-8"))?;
        equiv_core::parse_contract(&src).map_err(|e| format!("{path}: {e:?}"))
    } else {
        equiv_core::decode_contract(&bytes).map_err(|e| format!("{path}: {e:?}"))
    }
}

fn usage() -> &'static str {
    "equiv: deterministic checker for behaviour-preserving code changes

USAGE:
  equiv review <cand.py> <ref.py> <fn> <sig> [n] [seed] [--sign] [--all]
               [--inputs-from <file|dir>]... [--errors-agree]
        Check a function against a reference on generated inputs. <sig> is the
        argument types (int|str|list[int]|list[str]|float|dict|bool), comma separated. Emits a
        reproducible receipt. With --sign, signs it using the seed in
        $EQUIV_SIGNING_KEY (64 hex chars). A counterexample is followed by how
        many judged inputs disagreed; --all lists every one, in case order.
        --inputs-from also tries the literal arguments of every call to <fn>
        (and pytest parametrize rows) in those Python files; they are parsed,
        never run, and bound into the receipt.
        --errors-agree (parity mode, for a port and its reference): when both
        versions raise, count it as agreement whatever the exception class.
        One raising while the other returns is still a difference. The mode is
        printed and bound into the receipt.
  equiv review-pr <manifest> | --auto  [--base REF] [--receipts-out F] [--attest-out DIR]
               [--no-harvest]
        Check every listed (or auto-detected) changed function against base.
        Literal arguments of calls to each function anywhere in the repo's
        tracked Python files (tests included) are tried as extra inputs,
        unless --no-harvest.

METRICS:
  Off by default. With $EQUIV_METRICS=on, review-pr appends one line of counts
  (no code, names, paths or values) to ./equiv-metrics.jsonl. Nothing is sent
  anywhere. $DO_NOT_TRACK=1 overrides.

EVALUATOR ENVIRONMENT:
  The Python process that runs the functions gets only PATH, HOME, temp dirs,
  locale, PYTHON*, virtualenv/conda and Windows system variables. Credentials
  such as $EQUIV_SIGNING_KEY or $GH_TOKEN are not passed. Not a sandbox.

EVIDENCE:
  If every generated input (10 or more) makes both versions raise, only errors
  were compared: could not check (exit 2), never EQUIVALENT. If fewer than 10
  inputs returned a value, EQUIVALENT is labelled thin evidence, in the output
  and in the receipt (tier tested-thin). Functions with fewer than 10 possible
  inputs (no arguments, a bool) are checked completely and exempt.

SAFETY LIMIT:
  $EQUIV_SAFETY_SECONDS (default 120) caps the wall-clock time of one
  function's check, for long runs inside C code the step budget cannot count.
  A check that hits it is could-not-check (exit 2) with NO receipt: the limit
  is not part of the deterministic budget and never decides a verdict.
  equiv keygen
        Generate an ed25519 keypair; prints the seed and public key. The seed
        is never written to disk; store it in your keychain.
  equiv verify-receipt <signed-receipt-hex>
        Verify a signed receipt; prints the receipt id and signer.
  equiv fmt  <contract.eqct> [-o out.eqc]   compile contract text to canonical bytes
  equiv hash <contract.{eqct,eqc}>          print the contract's SHA-256 identity
  equiv lint <contract.{eqct,eqc}>          parse + validate, report errors
  equiv check <artifact.wasm> <contract>    (WASM path) check artifact against contract

EXIT CODES (frozen): 0 pass · 1 counterexample · 2 unknown · 3 invalid input"
}

fn run() -> Result<u8, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("review-pr") => {
            let safety = safety_seconds()?;
            let budget = step_budget(&args)?;
            let same_host = args.iter().any(|a| a == "--same-host");
            use equiv_review::{
                detect_changed_functions, render_markdown_with_unchecked, run_pr_with_inputs, ArgType,
                DetectedStatus, PrCheck, ReviewSpec,
            };
            let base = args
                .iter()
                .position(|a| a == "--base")
                .and_then(|i| args.get(i + 1).cloned())
                .or_else(|| std::env::var("EQUIV_BASE_REF").ok())
                .or_else(|| std::env::var("GITHUB_BASE_REF").ok())
                .unwrap_or_else(|| "origin/main".to_string());
            let auto = args.iter().any(|a| a == "--auto");

            let mut checks: Vec<PrCheck> = Vec::new();
            // Functions that changed but could not be checked. Surfaced in the
            // comment so a green result never hides them (closes issue #1).
            let mut unchecked: Vec<(String, String)> = Vec::new();

            // The base version of each function goes to a temp file owned by
            // this process. It used to be named after the function and file
            // only, so two reviews on one machine overwrote each other's base
            // and one could judge against the other's code. The process id
            // makes the path unique per run; (name, file) is unique within it.
            let pid = std::process::id();
            let tmp_base = |name: &str, file: &str, src: &[u8]| -> Result<std::path::PathBuf, String> {
                let slug: String = file.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let p = std::env::temp_dir().join(format!("equiv_base_{pid}_{name}_{slug}.py"));
                std::fs::write(&p, src).map_err(|e| e.to_string())?;
                Ok(p)
            };

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
                // merge-base (no repo, unrelated histories) falls back to the
                // ref as given, and the diff below reports the failure.
                let diff_base = std::process::Command::new("git")
                    .args(["merge-base", &base, "HEAD"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| base.clone());
                // --name-status with rename detection: a renamed file is the
                // same functions under a new path, so its base version is read
                // from the old path and its changed functions are checked
                // instead of being listed as "new" (plus "removed").
                let listed = std::process::Command::new("git")
                    .args(["-c", "core.quotepath=false", "diff", "-M", "--name-status", &diff_base, "--", "*.py"])
                    .output()
                    .map_err(|e| format!("git diff: {e}"))?;
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
                    let file = file.as_str();
                    let head_src = std::fs::read_to_string(file).unwrap_or_default();
                    let base_out = std::process::Command::new("git")
                        .args(["-c", "core.quotepath=false", "show", &format!("{diff_base}:{base_file}")])
                        .output();
                    let base_src = match base_out {
                        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
                        _ => String::new(),
                    };
                    for d in detect_changed_functions(&base_src, &head_src)? {
                        match d.status {
                            DetectedStatus::Checkable { args } => {
                                let base_path = tmp_base(&d.name, file, base_src.as_bytes())?;
                                checks.push(PrCheck {
                                    name: d.name.clone(),
                                    head_path: std::path::PathBuf::from(file),
                                    base_path,
                                    spec: ReviewSpec { func: d.name, args, n: equiv_review::DEFAULT_N, seed: 1, budget, errors_agree: false, domains: vec![], same_host },
                                });
                            }
                            DetectedStatus::NotChecked { reason } => unchecked.push((d.name, reason)),
                        }
                    }
                }
            } else {
                let manifest = args.get(1).ok_or("review-pr: missing manifest path (or pass --auto)")?;
                let text = std::fs::read_to_string(manifest).map_err(|e| format!("{manifest}: {e}"))?;
                for (lineno, line) in text.lines().enumerate() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    // file : function : sig[ : n[ : seed]]
                    let parts: Vec<&str> = line.split(':').map(|s| s.trim()).collect();
                    if parts.len() < 3 {
                        return Err(format!("{manifest}:{}: expected `file : fn : sig`", lineno + 1));
                    }
                    let (file, func, sig) = (parts[0], parts[1], parts[2]);
                    let n: u32 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(equiv_review::DEFAULT_N);
                    let seed: u64 = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
                    let arg_types = sig
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| ArgType::parse_domain(s).ok_or_else(|| format!("bad arg type `{}`", s.trim())))
                        .collect::<Result<Vec<_>, _>>()?;
                    let (arg_types, domains): (Vec<ArgType>, Vec<equiv_review::Domain>) = arg_types.into_iter().unzip();
                    let base_out = std::process::Command::new("git")
                        .args(["-c", "core.quotepath=false", "show", &format!("{base}:{file}")])
                        .output();
                    let base_bytes = match base_out {
                        Ok(o) if o.status.success() => o.stdout,
                        _ => Vec::new(),
                    };
                    let base_path = tmp_base(func, file, &base_bytes)?;
                    checks.push(PrCheck {
                        name: func.to_string(),
                        head_path: std::path::PathBuf::from(file),
                        base_path,
                        spec: ReviewSpec { func: func.to_string(), args: arg_types, n, seed, budget, errors_agree: false, domains, same_host },
                    });
                }
            }

            if checks.is_empty() && unchecked.is_empty() {
                eprintln!("review-pr: nothing changed to review");
                return Ok(0);
            }
            // Inputs harvested from the repository's own calls and tests
            // (0.17.0): one pass over every tracked Python file, at head.
            // `--no-harvest` turns it off.
            let harvested: Vec<Vec<equiv_review::Slots>> = if args.iter().any(|a| a == "--no-harvest") {
                Vec::new()
            } else {
                let wanted: Vec<(String, std::path::PathBuf, Vec<ArgType>)> =
                    checks.iter().map(|c| (c.spec.func.clone(), c.head_path.clone(), c.spec.args.clone())).collect();
                let files = equiv_review::harvest::python_files(std::path::Path::new("."));
                equiv_review::harvest::harvest(&wanted, &files)
            };
            let (items, code) = run_pr_with_inputs(&checks, &harvested, safety);
            // A changed function that auto-detection could not check is "not
            // checked" like a safety stop or a refusal: nothing diverged, but
            // the run is not a clean pass (0). Exit 2 says so; whether that
            // blocks the merge is the Action's `fail-on-not-checked` input.
            let code = if code == 0 && !unchecked.is_empty() { 2 } else { code };

            // Local-first verdict-rate metrics (fail-silent, counts only, no
            // network). This build reviews Python only, so the tag is "python".
            telemetry::record(&items, &unchecked, "python");

            // Sign if a key is configured (CI secret). Absent key => unsigned,
            // never an error: fork PRs legitimately have no secret access.
            let key = match std::env::var("EQUIV_SIGNING_KEY") {
                Ok(s) if !s.trim().is_empty() => Some(
                    equiv_core::SigningKey::from_hex(&s)
                        .map_err(|e| format!("bad EQUIV_SIGNING_KEY: {e:?}"))?,
                ),
                _ => None,
            };
            let signer_hex = key.as_ref().map(|k| hex(&k.public_key()));

            if let Some(k) = &key {
                // Durable, signed artifact (one TSV line per function): the
                // receipt belongs in CI artifact storage, not just the comment.
                let out = args
                    .iter()
                    .position(|a| a == "--receipts-out")
                    .and_then(|i| args.get(i + 1).cloned());
                let mut buf = String::new();
                // A safety stop has no receipt, so nothing is signed for it.
                for (it, r) in items.iter().filter_map(|it| it.outcome.as_ref().ok().map(|r| (it, r))) {
                    let signed = equiv_core::SignedReceipt::sign(r.to_bytes(), k);
                    buf.push_str(&format!(
                        "{}\t{}\t{}\n",
                        it.name,
                        hex(&signed.receipt_id()),
                        hex(&signed.to_bytes())
                    ));
                }
                if let Some(p) = out {
                    std::fs::write(&p, &buf).map_err(|e| format!("{p}: {e}"))?;
                }
            }

            // in-toto attestations (DR-3): one Statement per function, written
            // for the keyless Sigstore path (cosign signs them in CI).
            if let Some(i) = args.iter().position(|a| a == "--attest-out") {
                let dir = args.get(i + 1).ok_or("--attest-out needs a directory")?;
                std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
                for (check, item) in checks.iter().zip(items.iter()) {
                    // No receipt (safety stop) => nothing to attest.
                    let Ok(receipt) = &item.outcome else { continue };
                    let file = check.head_path.display().to_string();
                    let stmt = equiv_review::intoto_statement(&file, &item.name, receipt);
                    let path = std::path::Path::new(dir).join(format!("{}.intoto.json", item.name));
                    std::fs::write(&path, stmt).map_err(|e| format!("{}: {e}", path.display()))?;
                }
            }

            print!("{}", render_markdown_with_unchecked(&items, signer_hex.as_deref(), &unchecked));
            Ok(code as u8)
        }
        Some("keygen") => {
            let key = equiv_core::SigningKey::generate();
            println!("seed:       {}", hex(&key.seed()));
            println!("public-key: {}", hex(&key.public_key()));
            eprintln!(
                "\nThe seed is SECRET. Store it in your keychain, e.g.:\n  \
                 security add-generic-password -s equiv-signing-key -a $USER -w <seed>\n  \
                 export EQUIV_SIGNING_KEY=$(security find-generic-password -s equiv-signing-key -w)\n\
                 Never commit it or write it into the repo."
            );
            Ok(0)
        }
        Some("verify-receipt") => {
            let hexstr = args.get(1).ok_or("verify-receipt: missing signed-receipt hex")?;
            let bytes = from_hex(hexstr)?;
            let sr = equiv_core::SignedReceipt::from_bytes(&bytes)
                .map_err(|e| format!("not a signed receipt: {e:?}"))?;
            if sr.verify() {
                println!("VALID signature");
                println!("receipt-id: {}", hex(&sr.receipt_id()));
                println!("signer:     {}", hex(&sr.public_key));
                Ok(0)
            } else {
                println!("INVALID signature");
                Ok(1)
            }
        }
        Some("review") => {
            use equiv_review::{review_with_inputs, ArgType, ReviewSpec, ReviewVerdict};
            let safety = safety_seconds()?;
            let budget = step_budget(&args)?;
            let same_host = args.iter().any(|a| a == "--same-host");
            let want_sign = args.iter().any(|a| a == "--sign");
            let want_all = args.iter().any(|a| a == "--all");
            let errors_agree = args.iter().any(|a| a == "--errors-agree");
            // --inputs-from <file|dir>, repeatable: harvest literal call
            // inputs from those Python files (0.17.0).
            let mut inputs_from: Vec<std::path::PathBuf> = Vec::new();
            let mut rest: Vec<&String> = Vec::new();
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                if a == "--inputs-from" {
                    let p = it.next().ok_or("--inputs-from needs a file or directory")?;
                    inputs_from.push(std::path::PathBuf::from(p));
                } else if a == "--budget" {
                    it.next(); // read by step_budget
                } else {
                    rest.push(a);
                }
            }
            let pos: Vec<&String> = rest
                .into_iter()
                .filter(|a| *a != "--sign" && *a != "--all" && *a != "--errors-agree" && *a != "--same-host")
                .collect();
            let cand = pos.first().ok_or("review: missing candidate .py")?;
            let refr = pos.get(1).ok_or("review: missing reference .py")?;
            let func = pos.get(2).ok_or("review: missing function name")?;
            let sig = pos.get(3).ok_or("review: missing signature (e.g. int,int)")?;
            let n: u32 = pos.get(4).map(|s| s.parse().unwrap_or(equiv_review::DEFAULT_N)).unwrap_or(equiv_review::DEFAULT_N);
            let seed: u64 = pos.get(5).map(|s| s.parse().unwrap_or(1)).unwrap_or(1);
            let arg_types = sig
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| ArgType::parse_domain(s).ok_or_else(|| format!("bad arg type `{}`", s.trim())))
                .collect::<Result<Vec<_>, _>>()?;
            let (arg_types, domains): (Vec<ArgType>, Vec<equiv_review::Domain>) = arg_types.into_iter().unzip();
            let spec = ReviewSpec { func: func.to_string(), args: arg_types, n, seed, budget, errors_agree, domains, same_host };
            let harvested: Vec<equiv_review::Slots> = if inputs_from.is_empty() {
                Vec::new()
            } else {
                let files = equiv_review::harvest::expand_paths(&inputs_from);
                let wanted = vec![(spec.func.clone(), std::path::PathBuf::from(cand), spec.args.clone())];
                equiv_review::harvest::harvest(&wanted, &files).pop().unwrap_or_default()
            };
            let r = match review_with_inputs(
                std::path::Path::new(cand),
                std::path::Path::new(refr),
                &spec,
                &harvested,
                safety,
            ) {
                Ok(r) => r,
                Err(stop) => {
                    // Wall-clock stop: could not check, and deliberately no
                    // receipt, signed or not (backlog 0.3b, gate P3).
                    println!("ERROR: {}", stop.reason());
                    return Ok(stop.exit_code() as u8);
                }
            };
            match &r.verdict {
                ReviewVerdict::Equivalent { n, not_judged } => {
                    println!(
                        "EQUIVALENT: agreed on all {n} generated inputs (seed {seed}){}{}",
                        equiv_review::not_judged_note(*not_judged),
                        equiv_review::thin_note(&r).map(|t| format!("; {t}")).unwrap_or_default()
                    );
                }
                ReviewVerdict::Counterexample { input, candidate, reference, .. } => {
                    println!("COUNTEREXAMPLE at input {input}");
                    println!("  candidate -> {candidate}");
                    println!("  reference -> {reference}");
                    println!("{}", equiv_review::disagreements_note(&r.verdict));
                    if want_all {
                        for d in &r.disagreements {
                            println!("  at {}: candidate -> {}, reference -> {}", d.input, d.candidate, d.reference);
                        }
                    }
                }
                ReviewVerdict::Error { reason } => println!("ERROR: {reason}"),
                ReviewVerdict::Refused { reason } => println!("REFUSED: {reason}"),
            }
            println!("provenance: {}", equiv_review::provenance_note(&r));
            if !r.domains.is_empty() {
                println!("domains: {} (inputs generated only inside these; a pass says nothing outside them)", r.domains);
            }
            if same_host {
                println!("tier: {} (same host only: floats compared on this machine; another machine or maths library may give a different verdict)", r.tier);
            }
            if errors_agree {
                println!("mode: errors agree (both sides raising counts as agreement, whatever the exception class)");
            }
            println!("receipt-id: {}", hex(&r.sha256()));
            if want_sign {
                let seed = std::env::var("EQUIV_SIGNING_KEY")
                    .map_err(|_| "--sign needs $EQUIV_SIGNING_KEY (64 hex chars)")?;
                let key = equiv_core::SigningKey::from_hex(&seed)
                    .map_err(|e| format!("bad EQUIV_SIGNING_KEY: {e:?}"))?;
                let signed = equiv_core::SignedReceipt::sign(r.to_bytes(), &key);
                println!("signed-receipt: {}", hex(&signed.to_bytes()));
                println!("signer:         {}", hex(&signed.public_key));
            } else {
                println!("receipt: {}", hex(&r.to_bytes()));
            }
            Ok(r.verdict.exit_code() as u8)
        }
        Some("fmt") => {
            let path = args.get(1).ok_or("fmt: missing contract path")?;
            let c = load_contract(path)?;
            let bytes = equiv_core::encode_contract(&c);
            if let Some(i) = args.iter().position(|a| a == "-o") {
                let out = args.get(i + 1).ok_or("fmt: -o needs a path")?;
                std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
                eprintln!("wrote {} bytes, sha256 {}", bytes.len(), hex(&equiv_core::contract_sha256(&c)));
            } else {
                println!("{}", hex(&bytes));
            }
            Ok(0)
        }
        Some("hash") => {
            let path = args.get(1).ok_or("hash: missing contract path")?;
            let c = load_contract(path)?;
            println!("{}", hex(&equiv_core::contract_sha256(&c)));
            Ok(0)
        }
        Some("lint") => {
            let path = args.get(1).ok_or("lint: missing contract path")?;
            let c = load_contract(path)?;
            eprintln!(
                "ok: target `{}`, {} param(s), {} byte(s) canonical",
                c.target.name,
                c.target.params.len(),
                equiv_core::encode_contract(&c).len()
            );
            Ok(0)
        }
        Some("check") => {
            let artifact_path = args.get(1).ok_or("check: missing artifact path")?;
            let contract_path = args.get(2).ok_or("check: missing contract path")?;
            let artifact =
                std::fs::read(artifact_path).map_err(|e| format!("{artifact_path}: {e}"))?;
            let c = load_contract(contract_path)?;
            let receipt = equiv_engine::check(&artifact, &c, &equiv_engine::DiffConfig::default());
            println!("verdict: {:?}", receipt.verdict);
            println!("receipt: {}", hex(&receipt.to_bytes()));
            Ok(receipt.verdict.exit_code() as u8)
        }
        _ => {
            eprintln!("{}", usage());
            Ok(EXIT_INVALID)
        }
    }
}

fn main() -> ExitCode {
    #[cfg(unix)]
    forward_interrupts();
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(EXIT_INVALID)
        }
    }
}
