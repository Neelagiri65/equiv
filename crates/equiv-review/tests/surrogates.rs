//! Lone surrogates as str corners (path A step 3, corpus case 021). A Rust
//! String cannot hold one, so they are carried as UTF-16 code units.

use equiv_review::{review, ArgType, ReviewSpec, ReviewVerdict};
use std::io::Write;

fn tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("equiv_sur_{}_{name}.py", std::process::id()));
    std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
    p
}

fn spec(func: &str) -> ReviewSpec {
    ReviewSpec { func: func.into(), args: vec![ArgType::Str], n: 200, seed: 1, budget: equiv_review::DEFAULT_BUDGET, errors_agree: false, domains: vec![] }
}

#[test]
fn lenient_encoding_is_caught_at_a_lone_surrogate() {
    let c = tmp("c", "def f(s):\n    return len(s.encode('utf-8', errors='replace'))\n");
    let r = tmp("r", "def f(s):\n    return len(s.encode('utf-8'))\n");
    match review(&c, &r, &spec("f")).expect("no safety stop").verdict {
        ReviewVerdict::Counterexample { input, reference, .. } => {
            assert_eq!(input, "('\\ud800')");
            assert!(reference.contains("UnicodeEncodeError"), "{reference}");
        }
        v => panic!("expected a counterexample at a lone surrogate, got {v:?}"),
    }
}

#[test]
fn same_error_on_both_sides_stays_equivalent() {
    let c = tmp("ec", "def f(s):\n    return sum(len(ch.encode('utf-8')) for ch in s)\n");
    let r = tmp("er", "def f(s):\n    return len(s.encode('utf-8'))\n");
    let v = review(&c, &r, &spec("f")).expect("no safety stop").verdict;
    assert!(matches!(v, ReviewVerdict::Equivalent { .. }), "got {v:?}");
}

// Surrogate corners run after the random inputs (path A step 2 finding): a
// compiled twin that rejects lone surrogates must not hide an ordinary
// difference behind a '\ud800' headline. The surrogate difference is still
// counted among the disagreements.
#[test]
fn an_ordinary_difference_is_the_headline_before_surrogates() {
    let c = tmp("oc", "def f(s):\n    s.encode('utf-8')\n    return s.upper() if 'z' in s else s\n");
    let r = tmp("or", "def f(s):\n    return s\n");
    match review(&c, &r, &spec("f")).expect("no safety stop").verdict {
        ReviewVerdict::Counterexample { input, disagreeing, .. } => {
            assert!(!input.contains("\\ud") && input.contains('z'), "headline {input}");
            assert!(disagreeing > 3, "surrogates still counted: {disagreeing}");
        }
        v => panic!("expected a counterexample, got {v:?}"),
    }
}
