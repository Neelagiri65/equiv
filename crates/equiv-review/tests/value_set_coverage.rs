//! Value-set coverage (independent comparison, 2026-11-10): above 4,096 values
//! a list was sampled (about 38% of a 14k-name list) while the verdict still
//! read "agreed on all N generated inputs". A list is now judged completely up
//! to a large cap, and above it the receipt says how many values were tried.
//! Written before the change (holdout).
use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_vsc{}_{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn write(d: &Path, f: &str, body: &str) -> PathBuf {
    let p = d.join(f);
    std::fs::write(&p, body).unwrap();
    p
}

fn spec(sig: &str) -> ReviewSpec {
    let (t, dm) = ArgType::parse_domain(sig).unwrap();
    ReviewSpec { func: "f".into(), args: vec![t], n: 50, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains: vec![dm], same_host: false, hinted: false }
}

#[test]
fn a_6000_name_list_is_judged_completely() {
    // Every name is judged, so the one differing name is found whatever its
    // position, and an agreeing pair is judged on all 6,000.
    let d = dir("six");
    write(&d, "names.py", "NAMES = ['n%d' % i for i in range(6000)]\n");
    let r = write(&d, "r.py", "def f(s):\n    return 0\n");
    let same = write(&d, "same.py", "def f(s):\n    return 0\n");
    let all = review(&same, &r, &spec("str[in names.NAMES]")).expect("no safety stop");
    match all.verdict {
        ReviewVerdict::Equivalent { n, .. } => assert!(n >= 6000, "judged {n} of 6000 names"),
        v => panic!("{v:?}"),
    }
    assert!(!all.domains.contains("tried"), "complete coverage needs no sampling note: {}", all.domains);
    // Several positions, none named by a literal.
    for k in [1, 2047, 3001, 4999, 5998] {
        let c = write(&d, &format!("c{k}.py"), &format!("def f(s):\n    return 1 if s == 'n' + str({k} + 0) else 0\n"));
        let rec = review(&c, &r, &spec("str[in names.NAMES]")).expect("no safety stop");
        match rec.verdict {
            ReviewVerdict::Counterexample { input, .. } => assert!(input.contains(&format!("n{k}")), "{k}: {input}"),
            v => panic!("{k}: {v:?}"),
        }
    }
}

#[test]
fn a_list_above_the_cap_says_it_was_sampled() {
    let d = dir("big");
    write(&d, "names.py", "NAMES = ['n%d' % i for i in range(100500)]\n");
    let c = write(&d, "c.py", "def f(s):\n    return len(s)\n");
    let r = write(&d, "r.py", "def f(s):\n    return len(s)\n");
    let rec = review(&c, &r, &spec("str[in names.NAMES]")).expect("no safety stop");
    assert!(matches!(rec.verdict, ReviewVerdict::Equivalent { .. }), "{:?}", rec.verdict);
    assert!(rec.domains.contains("100500 values") && rec.domains.contains("tried"), "{}", rec.domains);
    let note = equiv_review::provenance_note(&rec);
    assert!(note.contains("tried"), "the provenance (CLI and PR comment) says so: {note}");
}
