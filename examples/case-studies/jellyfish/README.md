# jellyfish case study: files

Write-up: [`docs/case-studies/jellyfish.md`](../../../docs/case-studies/jellyfish.md).

- `ref.py` / `cand.py`: thin wrappers. `ref.py` calls jellyfish's pure-Python
  functions, `cand.py` the Rust ones.
- `gref.py` / `gcand.py`: the same for Metaphone and the match-rating codex,
  limited to ASCII letters and spaces (both sides get the same guard).
- `inside.sh`: every check, run inside the container.
- `run.sh`: downloads equiv v0.14.0 and the jellyfish 1.2.1 and 1.3.0 wheels,
  then runs `inside.sh` in a pinned `python:3.12` image with `--network none`.
  Needs `docker`, `gh` and `python3 -m pip`.
- `out-1.2.1.txt` / `out-1.3.0.txt`: the published output, with receipt ids.
  `run.sh` writes `rerun-*.txt`; the receipt ids should match.
