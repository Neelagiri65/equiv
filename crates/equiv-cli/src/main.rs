//! equiv CLI (v0 skeleton). One binary, every channel (DR-1): `check` is the
//! CLI/hook surface; `fmt`/`hash`/`lint` are the format toolchain. Exit codes
//! are frozen per DR-7: 0 pass, 1 counterexample, 2 unknown, 3 invalid input.

use std::process::ExitCode;

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
  equiv review <cand.py> <ref.py> <fn> <sig> [n] [seed] [--sign]
        Check a function against a reference on generated inputs. <sig> is the
        argument types (int|str|list[int]|float|dict), comma separated. Emits a
        reproducible receipt. With --sign, signs it using the seed in
        $EQUIV_SIGNING_KEY (64 hex chars).
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
            use equiv_review::{
                detect_changed_functions, render_markdown_with_unchecked, run_pr, ArgType,
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

            let tmp_base = |name: &str, file: &str, src: &[u8]| -> Result<std::path::PathBuf, String> {
                let slug: String = file.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let p = std::env::temp_dir().join(format!("equiv_base_{name}_{slug}.py"));
                std::fs::write(&p, src).map_err(|e| e.to_string())?;
                Ok(p)
            };

            if auto {
                // Detect changed functions from the diff. No manifest needed, so
                // a refactored function can no longer be silently left out.
                let listed = std::process::Command::new("git")
                    .args(["-c", "core.quotepath=false", "diff", "--name-only", &base, "--", "*.py"])
                    .output()
                    .map_err(|e| format!("git diff: {e}"))?;
                if !listed.status.success() {
                    return Err("git diff failed (need a git repo and a valid base ref)".into());
                }
                for file in String::from_utf8_lossy(&listed.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let head_src = std::fs::read_to_string(file).unwrap_or_default();
                    let base_out = std::process::Command::new("git")
                        .args(["-c", "core.quotepath=false", "show", &format!("{base}:{file}")])
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
                                    spec: ReviewSpec { func: d.name, args, n: 300, seed: 1 },
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
                    let n: u32 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
                    let seed: u64 = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
                    let arg_types = sig
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| ArgType::parse(s).ok_or_else(|| format!("bad arg type `{s}`")))
                        .collect::<Result<Vec<_>, _>>()?;
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
                        spec: ReviewSpec { func: func.to_string(), args: arg_types, n, seed },
                    });
                }
            }

            if checks.is_empty() && unchecked.is_empty() {
                eprintln!("review-pr: nothing changed to review");
                return Ok(0);
            }
            let (items, code) = run_pr(&checks);

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
                for it in &items {
                    let signed = equiv_core::SignedReceipt::sign(it.receipt.to_bytes(), k);
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
                    let file = check.head_path.display().to_string();
                    let stmt = equiv_review::intoto_statement(&file, item);
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
            use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
            let want_sign = args.iter().any(|a| a == "--sign");
            let pos: Vec<&String> = args.iter().skip(1).filter(|a| *a != "--sign").collect();
            let cand = pos.first().ok_or("review: missing candidate .py")?;
            let refr = pos.get(1).ok_or("review: missing reference .py")?;
            let func = pos.get(2).ok_or("review: missing function name")?;
            let sig = pos.get(3).ok_or("review: missing signature (e.g. int,int)")?;
            let n: u32 = pos.get(4).map(|s| s.parse().unwrap_or(200)).unwrap_or(200);
            let seed: u64 = pos.get(5).map(|s| s.parse().unwrap_or(1)).unwrap_or(1);
            let arg_types = sig
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| ArgType::parse(s).ok_or_else(|| format!("bad arg type `{s}`")))
                .collect::<Result<Vec<_>, _>>()?;
            let spec = ReviewSpec { func: func.to_string(), args: arg_types, n, seed };
            let r = review(std::path::Path::new(cand), std::path::Path::new(refr), &spec);
            match &r.verdict {
                ReviewVerdict::Equivalent { n } => {
                    println!("EQUIVALENT: agreed on all {n} generated inputs (seed {seed})");
                }
                ReviewVerdict::Counterexample { input, candidate, reference } => {
                    println!("COUNTEREXAMPLE at input {input}");
                    println!("  candidate -> {candidate}");
                    println!("  reference -> {reference}");
                }
                ReviewVerdict::Error { reason } => println!("ERROR: {reason}"),
                ReviewVerdict::Refused { reason } => println!("REFUSED: {reason}"),
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
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(EXIT_INVALID)
        }
    }
}
