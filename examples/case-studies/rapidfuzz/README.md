# rapidfuzz case study: files

Write-up: [`docs/case-studies/rapidfuzz.md`](../../../docs/case-studies/rapidfuzz.md).

- `ref.py` / `cand.py`: thin wrappers over rapidfuzz's pure-Python (`*_py`) and
  C++ (`*_cpp`) functions, results returned as they are. Functions: `fns-raw.txt`.
- `fref.py` / `fcand.py`: the numeric functions with both sides wrapped in
  `float()`, so Python's `0` and C++'s `0.0` compare equal. Functions: `fns-float.txt`.
- `inside.sh`: every check, run inside the container, plus labelled hand checks.
- `run.sh`: downloads equiv v0.15.0 and the rapidfuzz 3.14.6 wheel, then runs
  `inside.sh` in a pinned `python:3.12` image with `--network none`.
  Needs `docker`, `gh` and `python3 -m pip`.
- `classify.py`: sorts every listed disagreement into type only, last bits or
  value: `python3 classify.py out-3.14.6.txt`.
- `out-3.14.6.txt`: the published output, with receipt ids (linux-arm64).
  `run.sh` writes `rerun-3.14.6.txt`; the receipt ids should match.
