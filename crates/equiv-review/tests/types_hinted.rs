//! Argument types supplied by the caller (`hinted`). `equiv review` always takes
//! its types from the command line; when a wrapper could not read them from the
//! function's type hints and passed what the caller gave it, the receipt says
//! so: the tier gains `-hinted` after its base (`tested-hinted`,
//! `tested-same-host-hinted`, each with `-thin` after it when the evidence is
//! thin) and the mode is bound into the spec hash. Unhinted receipts keep
//! their bytes: the goldens in budget/floats/objects/tuples are not touched.
//! Written before the change (holdout).
use equiv_review::{provenance_note, review, thin_note, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_hinted{}_{name}", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn spec(args: Vec<ArgType>, same_host: bool, hinted: bool) -> ReviewSpec {
    ReviewSpec { func: "f".into(), args, n: 300, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![], same_host, hinted }
}

// Only the literal '#fff' (scanned from the source) reaches a returned value.
const THIN: &str = "def f(s):\n    if s == '#fff':\n        return 1\n    raise ValueError(s)\n";
// An opaque float helper: refused by default, judged under same-host.
const OPAQUE: &str = "def _h(x):\n    return x * 2.0\n\ndef f(x):\n    return _h(x)\n";
const PLAIN: &str = "def f(x):\n    return x + x\n";

#[test]
fn hinted_is_named_in_the_tier_and_bound_into_the_receipt() {
    let c = tmp("c.py", "def f(x):\n    return x + x\n");
    let r = tmp("r.py", "def f(x):\n    return x * 2\n");
    let plain = review(&c, &r, &spec(vec![ArgType::Int], false, false)).expect("no safety stop");
    let hinted = review(&c, &r, &spec(vec![ArgType::Int], false, true)).expect("no safety stop");
    assert!(matches!(hinted.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", hinted.verdict);
    assert_eq!(plain.verdict, hinted.verdict, "the label changes, never the verdict");
    assert_eq!(plain.tier, "tested");
    assert_eq!(hinted.tier, "tested-hinted");
    assert_ne!(plain.spec_sha256, hinted.spec_sha256, "the mode is bound into the spec hash");
    assert_ne!(plain.sha256(), hinted.sha256());
    assert!(provenance_note(&hinted).ends_with("tier tested-hinted"), "{}", provenance_note(&hinted));
    let again = review(&c, &r, &spec(vec![ArgType::Int], false, true)).expect("no safety stop");
    assert_eq!(again.sha256(), hinted.sha256(), "a hinted receipt is reproducible");
}

#[test]
fn hinted_comes_after_the_base_and_before_thin() {
    let p = tmp("thin.py", THIN);
    let thin = review(&p, &p, &spec(vec![ArgType::Str], false, true)).expect("no safety stop");
    assert!(thin_note(&thin).is_some(), "fixture must be thin: {:?}", thin.verdict);
    assert_eq!(thin.tier, "tested-hinted-thin");
    let thin_sh = review(&p, &p, &spec(vec![ArgType::Str], true, true)).expect("no safety stop");
    assert_eq!(thin_sh.tier, "tested-same-host-hinted-thin");
}

#[test]
fn same_host_is_still_said_loudly_when_hinted() {
    let c = tmp("o_c.py", OPAQUE);
    let r = tmp("o_r.py", PLAIN);
    let rec = review(&c, &r, &spec(vec![ArgType::Float], true, true)).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
    assert_eq!(rec.tier, "tested-same-host-hinted");
    assert!(provenance_note(&rec).contains("SAME HOST ONLY"), "{}", provenance_note(&rec));
}
