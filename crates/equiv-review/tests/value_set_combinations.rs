//! What a value-set review says it tried (0.28.0). With two `str[in ...]`
//! arguments about 2,000 of 90,000 pairs were tried while the output read
//! "(300 values)" for each argument and nothing else; and a
//! `list[str[in ...]]` argument read "(300 values)" although only the first
//! and last value were put in a list on purpose. The combinations are still
//! sampled; the output now says so, with the numbers, and a list argument
//! holds every value in at least one list. The lists live in a separate
//! module so no name is a literal of the reviewed source.
//! (require python3 on PATH)
use equiv_review::{provenance_note, review, ArgType, Domain, ReviewReceipt, ReviewSpec, ReviewVerdict, DEFAULT_BUDGET};
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_vscomb{}_{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn write(d: &Path, f: &str, body: &str) -> PathBuf {
    let p = d.join(f);
    std::fs::write(&p, body).unwrap();
    p
}

fn run(c: &Path, r: &Path, sigs: &[&str], n: u32) -> ReviewReceipt {
    let (args, domains): (Vec<ArgType>, Vec<Domain>) = sigs.iter().map(|s| ArgType::parse_domain(s).unwrap()).unzip();
    let spec = ReviewSpec { func: "f".into(), args, n, seed: 1, budget: DEFAULT_BUDGET, errors_agree: false, domains, same_host: false };
    review(c, r, &spec).expect("no safety stop")
}

fn judged(rec: &ReviewReceipt) -> u32 {
    match rec.verdict {
        ReviewVerdict::Equivalent { n, .. } => n,
        ref v => panic!("expected equivalent, got {v:?}"),
    }
}

#[test]
fn sampled_pairs_across_two_value_sets_are_stated_with_their_numbers() {
    let d = dir("pairs");
    write(&d, "names.py", "NAMES = ['n%03d' % i for i in range(300)]\n");
    let src = "def f(a, b):\n    return a < b\n";
    let (c, r) = (write(&d, "c.py", src), write(&d, "r.py", src));
    let rec = run(&c, &r, &["str[in names.NAMES]", "str[in names.NAMES]"], 2000);
    let n = judged(&rec);
    // Each argument's 300 values were all tried, as before...
    assert_eq!(rec.domains.matches("(300 values)").count(), 2, "{}", rec.domains);
    // ...and the pairs were sampled: every input is a distinct pair here.
    let want = format!("combinations across value-set arguments sampled: only {n} of 90000 tried");
    assert!(rec.domains.ends_with(&want), "{}", rec.domains);
    assert!(n < 90000);
    assert!(provenance_note(&rec).contains(&want), "the provenance line carries it: {}", provenance_note(&rec));
}

#[test]
fn completely_covered_combinations_need_no_note() {
    let d = dir("small");
    write(&d, "names.py", "NAMES = ['a' + 'x', 'b' + 'y']\n");
    let src = "def f(a, b):\n    return a < b\n";
    let (c, r) = (write(&d, "c.py", src), write(&d, "r.py", src));
    let rec = run(&c, &r, &["str[in names.NAMES]", "str[in names.NAMES]"], 200);
    assert_eq!(judged(&rec), 4, "two values each: four pairs");
    assert!(!rec.domains.contains("combinations"), "{}", rec.domains);
    // One value-set argument beside a plain one: nothing to combine.
    let src = "def f(a, n):\n    return a + str(n)\n";
    let (c, r) = (write(&d, "c1.py", src), write(&d, "r1.py", src));
    let rec = run(&c, &r, &["str[in names.NAMES]", "int"], 200);
    assert!(!rec.domains.contains("combinations") && !rec.domains.contains("tried"), "{}", rec.domains);
}

#[test]
fn three_value_sets_report_the_product() {
    let d = dir("three");
    write(&d, "names.py", "NAMES = ['n%02d' % i for i in range(40)]\n");
    let src = "def f(a, b, c):\n    return a + b + c\n";
    let (c, r) = (write(&d, "c.py", src), write(&d, "r.py", src));
    let rec = run(&c, &r, &["str[in names.NAMES]"; 3], 500);
    assert!(rec.domains.contains("of 64000 tried"), "{}", rec.domains);
}

#[test]
fn a_list_argument_holds_every_value_in_some_list_and_says_what_was_sampled() {
    let d = dir("list");
    write(&d, "names.py", "NAMES = ['n%d' % i for i in range(2000)]\n");
    let r = write(&d, "r.py", "def f(xs):\n    return 0\n");
    let same = write(&d, "same.py", "def f(xs):\n    return 0\n");
    let rec = run(&same, &r, &["list[str[in names.NAMES]]"], 50);
    assert!(judged(&rec) >= 2000, "{:?}", rec.verdict);
    assert!(
        rec.domains.contains("(2000 values; each tried in at least one list; combinations of values within a list sampled)"),
        "{}",
        rec.domains
    );
    // Several positions, none named by a literal: each is in some list.
    for k in [1, 1047, 1998] {
        let c = write(&d, &format!("c{k}.py"), &format!("def f(xs):\n    return 1 if 'n' + str({k} + 0) in xs else 0\n"));
        let rec = run(&c, &r, &["list[str[in names.NAMES]]"], 50);
        match rec.verdict {
            ReviewVerdict::Counterexample { input, .. } => assert!(input.contains(&format!("'n{k}'")), "{k}: {input}"),
            v => panic!("{k}: {v:?}"),
        }
    }
}

#[test]
fn a_scalar_value_set_keeps_its_plain_wording() {
    let d = dir("scalar");
    write(&d, "names.py", "NAMES = ['n%d' % i for i in range(30)]\n");
    let src = "def f(a):\n    return a\n";
    let (c, r) = (write(&d, "c.py", src), write(&d, "r.py", src));
    let rec = run(&c, &r, &["str[in names.NAMES]"], 50);
    assert_eq!(rec.domains, "arg 1 str[in names.NAMES] (30 values)");
}
