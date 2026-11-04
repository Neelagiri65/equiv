//! Constraint: code under review must not see the credentials equiv was given.
//! The evaluator runs the candidate and reference functions; before this fix it
//! inherited equiv's whole environment, so a candidate could read
//! `EQUIV_SIGNING_KEY`, `GH_TOKEN` or the Actions OIDC request token, and even
//! print one into the PR comment through its return value. The evaluator now
//! gets an allowlisted environment. Python's own settings (PYTHONPATH, PATH)
//! must still reach it.
use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_equiv");
const FAKE: &str = "f00dfeedf00dfeedf00dfeedf00dfeedf00dfeedf00dfeedf00dfeedf00dfeed";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_cli_envtest_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SECRETS: &[&str] = &[
    "EQUIV_SIGNING_KEY",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_RUNTIME_TOKEN",
    "AWS_SECRET_ACCESS_KEY",
    "SOME_UNRELATED_SECRET",
];

#[test]
fn candidate_cannot_read_credentials_from_the_environment() {
    let d = dir("secrets");
    let keys = SECRETS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", ");
    std::fs::write(
        d.join("cand.py"),
        format!("import os\ndef f(s):\n    return ','.join(os.environ.get(k, 'absent') for k in [{keys}])\n"),
    )
    .unwrap();
    let absent = vec!["absent"; SECRETS.len()].join(",");
    std::fs::write(d.join("ref.py"), format!("def f(s):\n    return '{absent}'\n")).unwrap();
    let mut c = Command::new(BIN);
    c.args(["review", "cand.py", "ref.py", "f", "str", "20", "1"]).current_dir(&d).env("EQUIV_METRICS", "off");
    for k in SECRETS {
        c.env(k, FAKE);
    }
    let out = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains(FAKE) && !stderr.contains(FAKE), "a credential reached the evaluated code:\n{stdout}{stderr}");
    assert_eq!(out.status.code(), Some(0), "expected EQUIVALENT:\n{stdout}{stderr}");
}

#[test]
fn pythonpath_still_reaches_the_evaluator() {
    let d = dir("pythonpath");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("equiv_env_helper.py"), "def double(s):\n    return s + s\n").unwrap();
    std::fs::write(d.join("cand.py"), "from equiv_env_helper import double\ndef f(s):\n    return double(s)\n").unwrap();
    std::fs::write(d.join("ref.py"), "def f(s):\n    return s * 2\n").unwrap();
    let out = Command::new(BIN)
        .args(["review", "cand.py", "ref.py", "f", "str", "20", "1"])
        .current_dir(&d)
        .env("EQUIV_METRICS", "off")
        .env("PYTHONPATH", &lib)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "PYTHONPATH was dropped:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
