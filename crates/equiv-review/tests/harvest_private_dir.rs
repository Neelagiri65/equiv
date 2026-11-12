//! Harvest scratch files (0.28.0). The script and its argument files used to
//! be written into the shared temp directory under computable names and left
//! there. They now live in a private directory that is gone when `harvest`
//! returns. One test only: it points the process's temp directory elsewhere.
//! (require python3 on PATH)
use equiv_review::harvest::harvest;
use equiv_review::ArgType;

#[test]
fn harvest_leaves_nothing_in_the_temp_directory() {
    let scratch = std::env::temp_dir().join(format!("equiv_hvpriv_{}", std::process::id()));
    let (tmp, proj) = (scratch.join("tmp"), scratch.join("proj"));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    // SAFETY: this test binary holds this single test, so no other thread
    // reads the environment while it is changed.
    unsafe { std::env::set_var("TMPDIR", &tmp) };
    if std::env::temp_dir() != tmp {
        return; // a platform whose temp directory does not follow TMPDIR
    }
    std::fs::write(proj.join("cand.py"), "def f(n):\n    return n\n").unwrap();
    std::fs::write(proj.join("test_f.py"), "def test_f():\n    assert f(41) == 41\n").unwrap();
    let wanted = vec![("f".to_string(), proj.join("cand.py"), vec![ArgType::Int])];
    let h = harvest(&wanted, &[proj.join("test_f.py")]).pop().unwrap();
    assert_eq!(h.len(), 1, "the call was harvested: {h:?}");
    let left: Vec<String> =
        std::fs::read_dir(&tmp).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(left.is_empty(), "harvest left files behind: {left:?}");
}
