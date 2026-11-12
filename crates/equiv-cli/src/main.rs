//! equiv CLI (v0 skeleton). One binary, every channel (DR-1): `check` is the
//! CLI/hook surface; `fmt`/`hash`/`lint` are the format toolchain. Exit codes
//! are frozen per DR-7: 0 pass, 1 counterexample, 2 unknown, 3 invalid input.
//! `review-pr` adds 4: the base ref could not be read, so nothing was compared.

use std::process::ExitCode;

mod args;
mod pr;
mod receipt_view;
mod telemetry;

const EXIT_INVALID: u8 = 3;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    // ASCII first: the slicing below is by byte, and a multi-byte character
    // would otherwise split mid-character and panic.
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not hex: only the characters 0-9 and a-f are allowed".into());
    }
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
fn step_budget(flag: Option<&str>) -> Result<u64, String> {
    let raw = match flag {
        Some(v) => Some(v.to_string()),
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

/// The value of `--budget` in raw arguments, before the strict parse (which
/// then rejects a `--budget` with nothing after it).
fn budget_flag(args: &[String]) -> Option<&str> {
    args.iter().position(|a| a == "--budget").and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// The signing key in $EQUIV_SIGNING_KEY, for a run that was asked to sign.
fn signing_key() -> Result<equiv_core::SigningKey, String> {
    let seed = std::env::var("EQUIV_SIGNING_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or("--sign needs $EQUIV_SIGNING_KEY (64 hex chars); nothing was run")?;
    equiv_core::SigningKey::from_hex(&seed).map_err(|e| format!("bad EQUIV_SIGNING_KEY: {e:?}; nothing was run"))
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
               [--no-harvest] [--same-host] [--budget N]
        Check every listed (or auto-detected) changed function against base.
        The manifest is read from the base ref: every line listed at base runs
        as written at base, whatever this branch did to the file. Lines only
        in this branch's copy run too and are named as new. With no manifest
        at base, this branch's copy is used and the output says so. A listed
        function that existed at base and is gone here counts as a behaviour
        change (exit 1). A base ref that cannot be read is exit 4: nothing
        was compared.
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
  PYTHONOPTIMIZE, PYTHONWARNINGS, PYTHONDEVMODE and PYTHONINTMAXSTRDIGITS are
  not passed either: they change what code does and are not in the receipt.
  The candidate and the reference each run in their own Python process.

EVIDENCE:
  If every generated input makes both versions raise, only errors were
  compared: could not check (exit 2), never EQUIVALENT. If fewer than 10
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
  equiv verify-receipt <signed-receipt-hex> [--signer <public-key-hex>]
        Check a signed receipt: the payload must be an equiv receipt and the
        signature must match the public key carried inside it. That alone
        does not show who signed it (anyone can make a key); pass --signer
        with the public key you expect and any other signer fails (exit 1).
        Prints the receipt id, the signer and what the receipt says.
  equiv fmt  <contract.eqct> [-o out.eqc]   compile contract text to canonical bytes
  equiv hash <contract.{eqct,eqc}>          print the contract's SHA-256 identity
  equiv lint <contract.{eqct,eqc}>          parse + validate, report errors
  equiv check <artifact.wasm> <contract>    (WASM path) check artifact against contract

EXIT CODES (frozen): 0 pass · 1 counterexample · 2 unknown · 3 invalid input
  review-pr only: 4 the base ref could not be read (nothing was compared)
  --help / -h print this text and exit 0"
}

fn run() -> Result<u8, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Asked-for help is not an error. (No arguments at all still is.)
    if args.iter().any(|a| a == "--help" || a == "-h") || args.first().map(String::as_str) == Some("help") {
        println!("{}", usage());
        return Ok(0);
    }
    match args.first().map(String::as_str) {
        Some("review-pr") => {
            let safety = safety_seconds()?;
            let budget = step_budget(budget_flag(&args))?;
            pr::run(&args[1..], safety, budget)
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
            let a = args::parse("verify-receipt", &args[1..], &[], &["--signer"])?;
            let hexstr = match a.pos.as_slice() {
                [h] => h,
                [] => return Err("verify-receipt: missing signed-receipt hex".into()),
                [_, extra, ..] => return Err(format!("verify-receipt: unexpected argument `{extra}`")),
            };
            let pinned = match a.value("--signer") {
                Some(s) => Some(
                    from_hex(s)
                        .ok()
                        .filter(|b| b.len() == 32)
                        .ok_or("verify-receipt: --signer must be a public key of 64 hex characters")?,
                ),
                None => None,
            };
            let bytes = from_hex(hexstr).map_err(|e| format!("verify-receipt: the signed receipt is {e}"))?;
            let sr = equiv_core::SignedReceipt::from_bytes(&bytes)
                .map_err(|e| format!("not a signed receipt: {e:?}"))?;
            // A signature over bytes that are not a receipt says nothing
            // about any review, whoever made it.
            let view = receipt_view::decode(&sr.receipt).map_err(|e| format!("not an equiv receipt: {e}"))?;
            if !sr.verify() {
                println!("INVALID signature");
                return Ok(1);
            }
            match &pinned {
                Some(want) if want.as_slice() != sr.public_key.as_slice() => {
                    println!("SIGNER MISMATCH: the signature is by a different key than the one given with --signer");
                    println!("signer:     {}", hex(&sr.public_key));
                    println!("expected:   {}", hex(want));
                    return Ok(1);
                }
                Some(_) => {
                    println!("signature:  matches the signer given with --signer");
                    println!("signer:     {}", hex(&sr.public_key));
                }
                None => {
                    println!("signature:  matches the public key carried inside this receipt");
                    println!("signer:     {} (not pinned)", hex(&sr.public_key));
                }
            }
            println!("receipt-id: {}", hex(&sr.receipt_id()));
            for line in &view.lines {
                println!("{line}");
            }
            if pinned.is_none() {
                println!(
                    "note: this does not show who signed the receipt. Anyone can make a key and sign any \
                     receipt with it. To accept only a signer you trust, pin its public key:\n  \
                     equiv verify-receipt <signed-receipt-hex> --signer <public-key-hex>"
                );
            }
            Ok(0)
        }
        Some("review") => {
            use equiv_review::{review_with_inputs, ReviewSpec, ReviewVerdict};
            let a = args::parse(
                "review",
                &args[1..],
                &["--sign", "--all", "--errors-agree", "--same-host"],
                &["--inputs-from", "--budget"],
            )?;
            let safety = safety_seconds()?;
            let budget = step_budget(a.value("--budget"))?;
            let same_host = a.has("--same-host");
            let want_all = a.has("--all");
            let errors_agree = a.has("--errors-agree");
            // --inputs-from <file|dir>, repeatable: harvest literal call
            // inputs from those Python files (0.17.0).
            let inputs_from: Vec<std::path::PathBuf> =
                a.values("--inputs-from").into_iter().map(std::path::PathBuf::from).collect();
            let pos = &a.pos;
            let cand = pos.first().ok_or("review: missing candidate .py")?;
            let refr = pos.get(1).ok_or("review: missing reference .py")?;
            let func = pos.get(2).ok_or("review: missing function name")?;
            let sig = pos.get(3).ok_or("review: missing signature (e.g. int,int)")?;
            let n: u32 = match pos.get(4) {
                Some(s) => args::number("review: the case count [n]", s)?,
                None => equiv_review::DEFAULT_N,
            };
            let seed: u64 = match pos.get(5) {
                Some(s) => args::number("review: the [seed]", s)?,
                None => 1,
            };
            if let Some(extra) = pos.get(6) {
                return Err(format!("review: unexpected argument `{extra}`"));
            }
            // The key is checked before anything runs: a run that is going to
            // fail must not print a verdict first.
            let key = if a.has("--sign") { Some(signing_key()?) } else { None };
            let (arg_types, domains) = pr::parse_sig(sig)?;
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
            if let Some(key) = &key {
                let signed = equiv_core::SignedReceipt::sign(r.to_bytes(), key);
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
            let checked = equiv_engine::check_explained(&artifact, &c, &equiv_engine::DiffConfig::default());
            let receipt = checked.receipt;
            println!("verdict: {:?}", receipt.verdict);
            // The reason for a could-not-check result: text for the reader,
            // not part of the receipt bytes.
            if let Some(note) = &checked.note {
                println!("reason: {note}");
            }
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
