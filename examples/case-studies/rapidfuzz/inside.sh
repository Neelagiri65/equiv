#!/bin/bash
# Runs inside python:3.12 with --network none. Wheels in /wheels, equiv in /equiv.
set -u
pip install --no-index --find-links /wheels "rapidfuzz==3.14.6" -q --root-user-action=ignore 2>/dev/null
export EQUIV_METRICS=off
cd /w
E=/equiv/equiv
echo "### rapidfuzz $(python3 -c 'import rapidfuzz;print(rapidfuzz.__version__)') / equiv 0.15.0 / $(python3 -c 'import platform;print(platform.machine())')"
echo "## A. raw wrappers (C++ result vs Python result as returned), seed 1"
while read f sig; do echo "== $f"; $E review cand.py ref.py $f $sig --all 2>&1; echo "exit=$?"; done < fns-raw.txt
echo "## B. both sides wrapped in float(), seeds 1 3 5"
while read f sig; do for seed in 1 3 5; do echo "== $f seed $seed"; $E review fcand.py fref.py $f $sig 2000 $seed --all 2>&1; echo "exit=$?"; done; done < fns-float.txt
echo "## C. hand checks (plain Python, not equiv output)"
python3 - <<'PY'
from rapidfuzz import fuzz_cpp as c, fuzz_py as p
from rapidfuzz.utils_cpp import default_process as dc
from rapidfuzz.utils_py import default_process as dp
a, b = "new york", "york new"
for fn in ("token_sort_ratio", "token_set_ratio", "token_ratio"):
    print(f"{fn}('new york', 'york<U+00A0>new'): C++ {getattr(c, fn)(a, b)}  Python {getattr(p, fn)(a, b)}")
print("'\\u00a0'.isspace():", " ".isspace())
print("default_process('\\u0130') code points: C++", [hex(ord(x)) for x in dc("İ")], "Python", [hex(ord(x)) for x in dp("İ")])
PY
