//! Value-set domains (charset_normalizer replay, 2026-11-10): many parity
//! functions take a string from a fixed list of names (encoding names, Unicode
//! range names). Generated junk ('' or a keyword) made three false blocks and
//! one junk headline in 48 real commits. `str[in mod.NAME]` draws strings only
//! from that list, resolved from the project's own code and bound into the
//! receipt. Written before the change (holdout).
use equiv_review::{review, ArgType, Domain, ReviewSpec, ReviewVerdict};
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_vs{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, f: &str, body: &str) -> PathBuf {
    let p = d.join(f);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, body).unwrap();
    p
}

fn spec(func: &str, sig: &[&str]) -> ReviewSpec {
    let (args, domains): (Vec<ArgType>, Vec<Domain>) =
        sig.iter().map(|s| ArgType::parse_domain(s).unwrap_or_else(|| panic!("parses: {s}"))).unzip();
    ReviewSpec { func: func.into(), args, n: 300, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains, same_host: false, hinted: false }
}

const NAMES: &str = "NAMES = ['Basic Latin', 'Latin-1 Supplement', 'Cyrillic', 'Greek and Coptic']\n";
// The twin raises on a name it does not know; the reference falls back.
const CAND: &str = "T = {'Basic Latin': 1, 'Latin-1 Supplement': 2, 'Cyrillic': 3, 'Greek and Coptic': 4}\ndef f(s):\n    return T[s]\n";
const REFR: &str = "T = {'Basic Latin': 1, 'Latin-1 Supplement': 2, 'Cyrillic': 3, 'Greek and Coptic': 4}\ndef f(s):\n    return T.get(s, 0)\n";

fn first_input(v: &ReviewVerdict) -> String {
    match v {
        ReviewVerdict::Counterexample { input, .. } => input.clone(),
        v => panic!("expected a counterexample, got {v:?}"),
    }
}

// --- parsing ---------------------------------------------------------------

#[test]
fn parses_value_sets() {
    let one = |s: &str| ArgType::parse_domain(s);
    assert_eq!(one("str[in names.NAMES]"), Some((ArgType::Str, Domain::OneOf { source: "names.NAMES".into(), values: vec![] })));
    assert_eq!(one("str[in .constant.IANA_SUPPORTED]"), Some((ArgType::Str, Domain::OneOf { source: ".constant.IANA_SUPPORTED".into(), values: vec![] })));
    assert_eq!(one("str[in NAMES]"), Some((ArgType::Str, Domain::OneOf { source: "NAMES".into(), values: vec![] })));
    assert_eq!(one("list[str[in names.NAMES]]"), Some((ArgType::ListStr, Domain::OneOf { source: "names.NAMES".into(), values: vec![] })));
    assert_eq!(one("str[in ]"), None);
    assert_eq!(one("str[in a b]"), None);
    assert_eq!(one("str[in a-b]"), None);
    assert_eq!(one("int[in names.NAMES]"), None);
}

// --- the replay shape ------------------------------------------------------

#[test]
fn unconstrained_leads_with_a_name_outside_the_list() {
    let d = dir("junk");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", CAND);
    let r = write(&d, "r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["str"])).expect("no safety stop");
    let input = first_input(&rec.verdict);
    assert!(!["Basic Latin", "Latin-1 Supplement", "Cyrillic", "Greek and Coptic"].iter().any(|n| input.contains(n)), "{input}");
}

#[test]
fn value_set_passes_twins_that_agree_on_every_name() {
    let d = dir("pass");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", CAND);
    let r = write(&d, "r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "got {:?}", rec.verdict);
    assert!(rec.domains.contains("str[in names.NAMES]") && rec.domains.contains("4 values"), "{}", rec.domains);
}

#[test]
fn value_set_leads_with_the_real_difference() {
    let d = dir("diff");
    write(&d, "names.py", NAMES);
    // Differs only on 'Cyrillic' (8 characters), with no literal naming it.
    let c = write(&d, "c.py", &CAND.replace("return T[s]", "return T[s] + (1 if len(s) == len('abcdefgh') else 0)"));
    let r = write(&d, "r.py", REFR);
    let rec = review(&c, &r, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    assert!(first_input(&rec.verdict).contains("Cyrillic"), "{:?}", rec.verdict);
}

#[test]
fn two_arguments_each_from_the_list() {
    let d = dir("pair");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", "T = {'Basic Latin': 1, 'Latin-1 Supplement': 2, 'Cyrillic': 3, 'Greek and Coptic': 4}\ndef f(a, b):\n    return T[a] < T[b]\n");
    let r = write(&d, "r.py", "T = {'Basic Latin': 1, 'Latin-1 Supplement': 2, 'Cyrillic': 3, 'Greek and Coptic': 4}\ndef f(a, b):\n    return T.get(a, 0) <= T.get(b, 0)\n");
    let rec = review(&c, &r, &spec("f", &["str[in names.NAMES]", "str[in names.NAMES]"])).expect("no safety stop");
    // They differ exactly when a == b: both arguments must be names.
    let input = first_input(&rec.verdict);
    assert_eq!(["Basic Latin", "Latin-1 Supplement", "Cyrillic", "Greek and Coptic"].iter().filter(|n| input.contains(*n)).count(), 1, "{input}");
}

#[test]
fn dict_keys_and_sets_are_value_sets() {
    let d = dir("kinds");
    write(&d, "names.py", "TABLE = {'utf_8': 1, 'cp1252': 2}\nSET = {'b', 'a'}\n");
    let c = write(&d, "c.py", "def f(s):\n    return {'utf_8': 1, 'cp1252': 2, 'a': 3, 'b': 4}[s]\n");
    let r = write(&d, "r.py", "def f(s):\n    return {'utf_8': 1, 'cp1252': 2, 'a': 3, 'b': 4}.get(s, 0)\n");
    for src in ["str[in names.TABLE]", "str[in names.SET]"] {
        let rec = review(&c, &r, &spec("f", &[src])).expect("no safety stop");
        assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{src}: {:?}", rec.verdict);
    }
}

#[test]
fn list_of_names() {
    let d = dir("list");
    write(&d, "names.py", NAMES);
    let c = write(&d, "c.py", &CAND.replace("def f(s):\n    return T[s]", "def f(xs):\n    return [T[s] for s in xs]"));
    let r = write(&d, "r.py", &REFR.replace("def f(s):\n    return T.get(s, 0)", "def f(xs):\n    return [T.get(s, 0) for s in xs]"));
    let plain = review(&c, &r, &spec("f", &["list[str]"])).expect("no safety stop");
    assert!(matches!(plain.verdict, ReviewVerdict::Counterexample { .. }), "{:?}", plain.verdict);
    let rec = review(&c, &r, &spec("f", &["list[str[in names.NAMES]]"])).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
}

#[test]
fn relative_to_the_candidate_package_and_bare_names() {
    // The charset_normalizer shape: `str[in .constant.NAMES]` from a module
    // inside a package, and a bare name defined in the candidate module.
    let d = dir("pkg");
    write(&d, "pkg/__init__.py", "");
    write(&d, "pkg/constant.py", NAMES);
    let c = write(&d, "pkg/mod.py", &format!("from .constant import NAMES\n{CAND}"));
    let r = write(&d, "ref.py", REFR);
    for src in ["str[in .constant.NAMES]", "str[in pkg.constant.NAMES]", "str[in NAMES]"] {
        let rec = review(&c, &r, &spec("f", &[src])).expect("no safety stop");
        assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{src}: {:?}", rec.verdict);
    }
}

// --- failure is loud --------------------------------------------------------

#[test]
fn an_unresolvable_or_unusable_list_is_could_not_check() {
    let d = dir("bad");
    write(&d, "names.py", "NUMS = [1, 2]\nEMPTY = []\nONE = 'Cyrillic'\n");
    let c = write(&d, "c.py", CAND);
    let r = write(&d, "r.py", REFR);
    for src in ["names.MISSING", "nosuchmodule.NAMES", "names.NUMS", "names.EMPTY", "names.ONE"] {
        let rec = review(&c, &r, &spec("f", &[&format!("str[in {src}]")])).expect("no safety stop");
        match &rec.verdict {
            ReviewVerdict::Error { reason } => assert!(reason.contains(src), "{src}: {reason}"),
            v => panic!("{src}: expected could-not-check, got {v:?}"),
        }
    }
}

// --- receipt ---------------------------------------------------------------

#[test]
fn the_resolved_values_are_bound_into_the_receipt() {
    // Same source name, different list contents: different receipt ids, so a
    // pass can never be re-read against another list.
    let a = dir("rid_a");
    let b = dir("rid_b");
    write(&a, "names.py", NAMES);
    write(&b, "names.py", "NAMES = ['Basic Latin', 'Cyrillic']\n");
    let body_c = "def f(s):\n    return len(s)\n";
    let body_r = "def f(s):\n    return len(s) + 0\n";
    let (ca, ra) = (write(&a, "c.py", body_c), write(&a, "r.py", body_r));
    let (cb, rb) = (write(&b, "c.py", body_c), write(&b, "r.py", body_r));
    let x = review(&ca, &ra, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    let y = review(&cb, &rb, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    let plain = review(&ca, &ra, &spec("f", &["str"])).expect("no safety stop");
    // The spec hash itself (not just the input counts) carries the values.
    assert_ne!(x.spec_sha256, y.spec_sha256);
    assert_ne!(x.spec_sha256, plain.spec_sha256);
}

#[test]
fn every_name_in_a_list_is_judged() {
    // A difference on one name out of 500 must be found even at n = 300: a
    // finite list is checked completely, not sampled.
    let d = dir("all");
    write(&d, "names.py", "NAMES = ['n%d' % i for i in range(500)]\n");
    let c = write(&d, "c.py", "def f(s):\n    return 1 if s == 'n' + str(250) else 0\n");
    let r = write(&d, "r.py", "def f(s):\n    return 0\n");
    let rec = review(&c, &r, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    assert!(first_input(&rec.verdict).contains("n250"), "{:?}", rec.verdict);
    let same = write(&d, "same.py", "def f(s):\n    return 0\n");
    let all = review(&same, &r, &spec("f", &["str[in names.NAMES]"])).expect("no safety stop");
    match all.verdict {
        ReviewVerdict::Equivalent { n, .. } => assert!(n >= 500, "judged {n} of 500 names"),
        v => panic!("got {v:?}"),
    }
}
