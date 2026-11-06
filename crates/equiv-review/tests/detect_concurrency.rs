//! Concurrent change detection on identical sources must not share temp files.
//! The detector once named its temp files by a hash of the sources alone, so
//! parallel runs on the same diff truncated and rewrote each other's files and
//! one could read a half-written file ("review-pr: nothing changed to review",
//! seen in CI on the telemetry tests, which run four identical repos at once).

use equiv_review::{detect_changed_functions, DetectedStatus};

#[test]
fn parallel_detection_on_identical_sources_is_stable() {
    // Long sources widen the truncate-then-write window.
    let pad: String = (0..4000).map(|i| format!("# padding line {i}\n")).collect();
    let base = format!("{pad}def f(x: int):\n    return x * 2\n");
    let head = format!("{pad}def f(x: int):\n    return x + x  # refactor\n");
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let (b, h) = (base.clone(), head.clone());
            std::thread::spawn(move || {
                (0..8)
                    .map(|_| match detect_changed_functions(&b, &h) {
                        Ok(d) if d.len() == 1 && d[0].name == "f" && matches!(d[0].status, DetectedStatus::Checkable { .. }) => None,
                        other => Some(format!("{other:?}")),
                    })
                    .find(Option::is_some)
                    .flatten()
            })
        })
        .collect();
    let bad: Vec<String> = handles.into_iter().filter_map(|h| h.join().unwrap()).collect();
    assert!(bad.is_empty(), "{} of 16 threads saw a wrong detection, e.g. {}", bad.len(), bad[0]);
}

#[test]
fn detection_leaves_no_temp_files_behind() {
    let marker = "# leftover-check 7f3a\n";
    let base = format!("{marker}def g(x: int):\n    return x\n");
    let head = format!("{marker}def g(x: int):\n    return x + 0\n");
    detect_changed_functions(&base, &head).unwrap();
    let pid = std::process::id().to_string();
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            // Only this test's files: the parallel test above shares the pid.
            n.starts_with("equiv_det")
                && n.contains(&format!("_{pid}_"))
                && std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("leftover-check 7f3a"))
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.is_empty(), "temp files left behind: {left:?}");
}
