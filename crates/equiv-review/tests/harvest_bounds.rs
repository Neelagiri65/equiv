//! Stacked `parametrize` decorators multiply (0.28.0). Their product used to
//! be built in full before any cap applied: four decorators of 60 rows are
//! 12,960,000 combinations. It is now cut to its first rows after every
//! decorator, in the order it was always built in, so a product under the
//! cap is harvested exactly as before and a huge one costs a bounded amount.
//! (require python3 on PATH)
use equiv_review::harvest::{harvest, HARVEST_CAP, PARAMETRIZE_ROWS_CAP};
use equiv_review::{ArgType, PyVal};
use std::path::PathBuf;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("equiv_hvbound_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const CAND: &str = "def f(a, b, c, d):\n    return a + b + c + d\n";

fn stacked(rows: usize) -> String {
    let vals: Vec<String> = (0..rows).map(|i| i.to_string()).collect();
    let list = vals.join(", ");
    format!(
        "import pytest\n\n@pytest.mark.parametrize('a', [{list}])\n@pytest.mark.parametrize('b', [{list}])\n\
         @pytest.mark.parametrize('c', [{list}])\n@pytest.mark.parametrize('d', [{list}])\n\
         def test_f(a, b, c, d):\n    assert f(a, b, c, d) is not None\n"
    )
}

fn wanted(d: &std::path::Path) -> Vec<(String, PathBuf, Vec<ArgType>)> {
    vec![("f".into(), d.join("cand.py"), vec![ArgType::Int; 4])]
}

#[test]
fn a_huge_stacked_product_is_cut_early() {
    let d = dir("huge");
    std::fs::write(d.join("cand.py"), CAND).unwrap();
    std::fs::write(d.join("test_f.py"), stacked(60)).unwrap();
    assert!(60usize.pow(4) > PARAMETRIZE_ROWS_CAP * 1000);
    let started = std::time::Instant::now();
    let h = harvest(&wanted(&d), &[d.join("test_f.py")]).pop().unwrap();
    // Generous: the full product takes minutes and gigabytes.
    assert!(started.elapsed().as_secs() < 30, "took {:?}", started.elapsed());
    assert_eq!(h.len(), HARVEST_CAP, "the first rows are still harvested");
    // The first rows of the product in its usual order: the first
    // decorator's value fixed, the last one running.
    let int = |n: i64| Some(PyVal::Int(n));
    assert_eq!(h[0], vec![int(0), int(0), int(0), int(0)]);
    assert_eq!(h[1], vec![int(0), int(0), int(0), int(1)]);
    assert_eq!(h[61], vec![int(0), int(0), int(1), int(1)]);
}

#[test]
fn a_product_under_the_cap_is_complete() {
    let d = dir("small");
    std::fs::write(d.join("cand.py"), CAND).unwrap();
    std::fs::write(d.join("test_f.py"), stacked(4)).unwrap();
    let h = harvest(&wanted(&d), &[d.join("test_f.py")]).pop().unwrap();
    assert_eq!(h.len(), 256, "4^4 rows, every one");
    let int = |n: i64| Some(PyVal::Int(n));
    assert_eq!(h[255], vec![int(3), int(3), int(3), int(3)]);
}
