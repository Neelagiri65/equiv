//! `equiv review --reference-python <exe> --candidate-python <exe> [--versions-of <dist>,..]`:
//! each side runs in the interpreter named for it, so two releases of a package
//! can be compared without path tricks, and each side states from inside its
//! own process what it ran. A receipt never names a version that did not run.
//! Written before the feature (holdout). Offline: the two environments are
//! made with `python3 -m venv --without-pip` and hold a hand-made package.
#![cfg(unix)]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use support::fresh_dir;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");

/// A virtual environment holding the distribution `demo` at `version`, whose
/// module is `module_src`. Returns the environment's interpreter.
fn env_with(root: &Path, name: &str, version: &str, module_src: &str) -> PathBuf {
    let venv = root.join(name);
    let made = Command::new("python3").args(["-m", "venv", "--without-pip"]).arg(&venv).output().unwrap();
    assert!(made.status.success(), "venv: {}", String::from_utf8_lossy(&made.stderr));
    let python = venv.join("bin/python");
    let site = Command::new(&python).args(["-c", "import sysconfig; print(sysconfig.get_paths()['purelib'])"]).output().unwrap();
    let site = PathBuf::from(String::from_utf8_lossy(&site.stdout).trim());
    std::fs::create_dir_all(site.join("demo")).unwrap();
    std::fs::write(site.join("demo/__init__.py"), module_src).unwrap();
    let info = site.join(format!("demo-{version}.dist-info"));
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("METADATA"), format!("Metadata-Version: 2.1\nName: demo\nVersion: {version}\n")).unwrap();
    std::fs::write(info.join("top_level.txt"), "demo\n").unwrap();
    std::fs::write(info.join("INSTALLER"), "test\n").unwrap();
    std::fs::write(
        info.join("RECORD"),
        format!("demo/__init__.py,,\ndemo-{version}.dist-info/METADATA,,\ndemo-{version}.dist-info/top_level.txt,,\ndemo-{version}.dist-info/INSTALLER,,\ndemo-{version}.dist-info/RECORD,,\n"),
    )
    .unwrap();
    python
}

const DOUBLE: &str = "NAMES = ['a', 'b']\ndef f(x):\n    return x * 2\ndef pick(name):\n    return name.upper()\n";
const DOUBLE_ADD: &str = "NAMES = ['a', 'b']\ndef f(x):\n    return x + x\ndef pick(name):\n    return name.upper()\n";
const BROKEN_AT_7: &str = "NAMES = ['a', 'b', 'c']\ndef f(x):\n    return x * 2 if x != 7 else 0\ndef pick(name):\n    return name.upper()\n";
/// The file under review: the same on both sides. It calls into the installed package.
const WRAP: &str = "import demo\n\ndef f(x: int) -> int:\n    return demo.f(x)\n\ndef g(x: int) -> int:\n    return x + 1\n\nNAMES = list(demo.NAMES)\n\ndef pick(name: str) -> str:\n    return demo.pick(name)\n";

struct Run {
    code: Option<i32>,
    out: String,
}

fn review(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let o = Command::new(BIN)
        .arg("review")
        .args(args)
        .current_dir(dir)
        .env("EQUIV_METRICS", "off")
        .env_remove("PYTHONPATH")
        .env_remove("VIRTUAL_ENV")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    Run { code: o.status.code(), out: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)) }
}

fn line<'a>(out: &'a str, starts: &str) -> &'a str {
    out.lines().find(|l| l.starts_with(starts)).unwrap_or_else(|| panic!("no line starting `{starts}` in:\n{out}"))
}

fn receipt_id(out: &str) -> String {
    line(out, "receipt-id:").to_string()
}

/// Two environments and the wrapper file, for one test.
fn setup(name: &str, reference_src: &str, candidate_src: &str, candidate_version: &str) -> (PathBuf, String, String) {
    let d = fresh_dir("equiv_sides", name);
    let r = env_with(&d, "ref", "1.0.0", reference_src);
    let c = env_with(&d, "cand", candidate_version, candidate_src);
    std::fs::write(d.join("w.py"), WRAP).unwrap();
    (d, r.display().to_string(), c.display().to_string())
}

#[test]
fn a_difference_between_two_releases_is_found_and_each_side_says_what_it_ran() {
    let (d, r, c) = setup("differs", DOUBLE, BROKEN_AT_7, "2.0.0");
    let run = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", &c, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(1), "{}", run.out);
    assert!(run.out.contains("COUNTEREXAMPLE") && run.out.contains("(7)"), "{}", run.out);
    let (rl, cl) = (line(&run.out, "reference:"), line(&run.out, "candidate:"));
    assert!(rl.contains("python 3.") && rl.contains("demo 1.0.0"), "{rl}");
    assert!(cl.contains("python 3.") && cl.contains("demo 2.0.0"), "{cl}");
}

#[test]
fn the_same_behaviour_in_two_releases_passes_and_the_receipt_binds_both_versions() {
    let (d, r, c) = setup("same", DOUBLE, DOUBLE_ADD, "2.0.0");
    let args = |cand: &str| vec!["w.py".to_string(), "w.py".into(), "f".into(), "int".into(), "--reference-python".into(), r.clone(), "--candidate-python".into(), cand.to_string(), "--versions-of".into(), "demo".into()];
    let a = args(&c);
    let first = review(&d, &a.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    assert_eq!(first.code, Some(0), "{}", first.out);
    assert!(first.out.contains("EQUIVALENT"), "{}", first.out);
    assert!(line(&first.out, "candidate:").contains("demo 2.0.0"), "{}", first.out);
    // Another release with the same code: the same verdict, a different receipt.
    let c3 = env_with(&d, "cand3", "3.0.0", DOUBLE_ADD).display().to_string();
    let b = args(&c3);
    let second = review(&d, &b.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    assert_eq!(second.code, Some(0), "{}", second.out);
    assert!(line(&second.out, "candidate:").contains("demo 3.0.0"), "{}", second.out);
    assert_ne!(receipt_id(&first.out), receipt_id(&second.out), "the versions that ran are part of the receipt");
}

#[test]
fn without_the_options_nothing_about_sides_is_printed() {
    let d = fresh_dir("equiv_sides", "plain");
    std::fs::write(d.join("a.py"), "def g(x: int) -> int:\n    return x + 1\n").unwrap();
    std::fs::write(d.join("b.py"), "def g(x: int) -> int:\n    return 1 + x\n").unwrap();
    // A variable of that name is not an option: only the flags select an interpreter.
    let run = review(&d, &["a.py", "b.py", "g", "int"], &[("EQUIV_REFERENCE_PYTHON", "/nonexistent/python"), ("EQUIV_CANDIDATE_PYTHON", "/nonexistent/python")]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    assert!(!run.out.lines().any(|l| l.starts_with("reference:") || l.starts_with("candidate:")), "{}", run.out);
}

#[test]
fn one_side_may_keep_the_default_interpreter() {
    let (d, _r, c) = setup("one_flag", DOUBLE, DOUBLE_ADD, "2.0.0");
    // g does not touch the package, so the default python3 can run the reference side.
    std::fs::write(d.join("plain.py"), "def g(x: int) -> int:\n    return x + 1\n").unwrap();
    let run = review(&d, &["w.py", "plain.py", "g", "int", "--candidate-python", &c], &[]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    assert!(line(&run.out, "reference:").contains("python 3."), "{}", run.out);
    assert!(line(&run.out, "candidate:").contains("python 3."), "{}", run.out);
}

#[test]
fn a_named_package_missing_on_a_side_is_could_not_check() {
    let (d, _r, c) = setup("missing", DOUBLE, DOUBLE_ADD, "2.0.0");
    std::fs::write(d.join("plain.py"), "def g(x: int) -> int:\n    return x + 1\n").unwrap();
    // The reference side is the default python3, which has no `demo`.
    let run = review(&d, &["w.py", "plain.py", "g", "int", "--candidate-python", &c, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(2), "{}", run.out);
    assert!(run.out.contains("demo") && run.out.contains("reference"), "it says which package and which side: {}", run.out);
    assert!(!run.out.contains("EQUIVALENT"), "{}", run.out);
}

#[test]
fn a_named_package_loaded_from_somewhere_else_is_could_not_check() {
    let (d, r, c) = setup("redirected", DOUBLE, DOUBLE_ADD, "2.0.0");
    // The file puts another copy of the package first on the import path: the
    // installed 2.0.0 is not what runs on either side.
    std::fs::create_dir_all(d.join("vendored/demo")).unwrap();
    std::fs::write(d.join("vendored/demo/__init__.py"), DOUBLE).unwrap();
    let redirect = format!("import sys\nsys.path.insert(0, {:?})\n{WRAP}", d.join("vendored").display().to_string());
    std::fs::write(d.join("w.py"), redirect).unwrap();
    let run = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", &c, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(2), "{}", run.out);
    assert!(!run.out.contains("EQUIVALENT") && !run.out.lines().any(|l| l.starts_with("receipt-id:")), "no receipt names a version that did not run: {}", run.out);
    assert!(run.out.contains("demo"), "{}", run.out);
}

#[test]
fn a_named_package_the_function_never_imports_is_could_not_check() {
    let (d, r, c) = setup("not_imported", DOUBLE, DOUBLE_ADD, "2.0.0");
    std::fs::write(d.join("plain.py"), "def g(x: int) -> int:\n    return x + 1\n").unwrap();
    let run = review(&d, &["plain.py", "plain.py", "g", "int", "--reference-python", &r, "--candidate-python", &c, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(2), "{}", run.out);
    assert!(run.out.contains("demo"), "{}", run.out);
    assert!(!run.out.lines().any(|l| l.starts_with("receipt-id:")), "{}", run.out);
}

#[test]
fn the_same_version_on_both_sides_is_allowed_and_said() {
    let (d, r, _c) = setup("same_version", DOUBLE, DOUBLE_ADD, "2.0.0");
    let patched = env_with(&d, "patched", "1.0.0", DOUBLE_ADD).display().to_string();
    let run = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", &patched, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(0), "a patched copy of the same version can be compared: {}", run.out);
    assert!(line(&run.out, "reference:").contains("demo 1.0.0") && line(&run.out, "candidate:").contains("demo 1.0.0"), "{}", run.out);
    assert!(run.out.contains("same version on both sides"), "{}", run.out);
}

#[test]
fn pythonpath_with_a_per_side_interpreter_is_refused() {
    let (d, r, c) = setup("pythonpath", DOUBLE, DOUBLE_ADD, "2.0.0");
    let run = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", &c], &[("PYTHONPATH", "/somewhere")]);
    assert_eq!(run.code, Some(3), "{}", run.out);
    assert!(run.out.contains("PYTHONPATH"), "{}", run.out);
}

#[test]
fn an_interpreter_that_does_not_exist_is_a_usage_error_and_one_that_is_not_python_is_could_not_check() {
    let (d, r, _c) = setup("bad_exe", DOUBLE, DOUBLE_ADD, "2.0.0");
    let gone = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", "/nonexistent/bin/python"], &[]);
    assert_eq!(gone.code, Some(3), "{}", gone.out);
    assert!(gone.out.contains("/nonexistent/bin/python"), "{}", gone.out);
    let not_python = review(&d, &["w.py", "w.py", "f", "int", "--reference-python", &r, "--candidate-python", "/usr/bin/true"], &[]);
    assert_eq!(not_python.code, Some(2), "{}", not_python.out);
    assert!(!not_python.out.contains("EQUIVALENT"), "{}", not_python.out);
}

#[test]
fn a_value_set_that_differs_between_the_sides_is_said() {
    // The reference release knows two names, the candidate three.
    let (d, r, c) = setup("domain", DOUBLE, BROKEN_AT_7, "2.0.0");
    let run = review(&d, &["w.py", "w.py", "pick", "str[in NAMES]", "--reference-python", &r, "--candidate-python", &c, "--versions-of", "demo"], &[]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    assert!(run.out.contains("3 values"), "the candidate's list is what is tried: {}", run.out);
    assert!(run.out.contains("only on the candidate side"), "the reference resolves the list differently, and the output says so: {}", run.out);
}

#[test]
fn the_options_belong_to_review_only() {
    let d = fresh_dir("equiv_sides", "review_only");
    let o = Command::new(BIN).args(["review-pr", "--auto", "--reference-python", "/usr/bin/python3"]).current_dir(&d).output().unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", String::from_utf8_lossy(&o.stderr));
}
