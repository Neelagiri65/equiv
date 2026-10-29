#!/usr/bin/env python3
"""Score `equiv review` against the divergence corpus in divergences/.

Two passes:

1. Corpus integrity (independent of equiv). For every `diverges` case the
   recorded witness is run through both versions and must produce different
   canonical results. For every `equivalent` case both versions are compared on
   an independent randomised input set (wider than equiv's own generator:
   unicode, long strings, very large ints, float corner bit patterns). A case
   that fails integrity is mislabelled and is reported as a corpus error.

2. Scoring. `equiv review` runs on every case; its exit code is compared to the
   label and to the checked-in baseline (divergences/BASELINE.json).

Exit status: 0 when there are no false alarms, no regressions against the
baseline and no corpus errors; 1 otherwise. Misses and could-not-check results
are reported, never failed: they are what the corpus measures.

Usage: run_divergences.py --bin PATH [--update-baseline] [--skip-integrity]
                          [--integrity-n N] [--only SUBSTRING]
"""

import argparse
import importlib.util
import json
import os
import platform
import random
import re
import struct
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "divergences"
BASELINE = CORPUS / "BASELINE.json"

# ---------------------------------------------------------------- canon
# Mirrors canon() in the equiv-review driver (crates/equiv-review/src/lib.rs),
# so "differs" here means the same thing it means to equiv: 42 and 42.0 differ,
# True and 1 differ, all NaNs are one value, dicts compare as maps.


def canon(x):
    if isinstance(x, bool):
        return repr(x)
    if isinstance(x, int):
        return repr(x)
    if isinstance(x, float):
        return "nanq" if x != x else x.hex()
    if isinstance(x, str):
        return repr(x)
    if x is None:
        return "None"
    if isinstance(x, list):
        return "[" + ", ".join(canon(e) for e in x) + "]"
    if isinstance(x, dict):
        if not all(isinstance(k, str) for k in x):
            return "__NONCANON__"
        return "{" + ", ".join(repr(k) + ": " + canon(x[k]) for k in sorted(x)) + "}"
    return "__NONCANON__"


def outcome(fn, args):
    try:
        return canon(fn(*args))
    except Exception as e:  # noqa: BLE001 - exception type is the observable
        return "raise " + type(e).__name__


# ---------------------------------------------------------------- cases


def load_cases(only=None):
    cases = []
    for d in sorted(p for p in CORPUS.iterdir() if p.is_dir()):
        if only and only not in d.name:
            continue
        meta = tomllib.loads((d / "case.toml").read_text(encoding="utf-8"))
        for k in ("function", "args", "expected", "class", "pattern"):
            if k not in meta:
                sys.exit(f"{d.name}: case.toml missing `{k}`")
        if meta["expected"] not in ("diverges", "equivalent"):
            sys.exit(f"{d.name}: expected must be 'diverges' or 'equivalent'")
        if meta["expected"] == "diverges" and "witness" not in meta:
            sys.exit(f"{d.name}: a diverges case needs a `witness`")
        meta["id"] = d.name
        meta["dir"] = d
        cases.append(meta)
    return cases


def load_fn(path, name, tag):
    spec = importlib.util.spec_from_file_location(f"_divcase_{tag}", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return getattr(mod, name)


def parse_witness(text):
    w = eval(text, {"__builtins__": {}, "float": float}, {})  # corpus-authored literal
    if not isinstance(w, tuple):
        raise ValueError("witness must be a tuple of arguments")
    return w


# ---------------------------------------------------------------- independent inputs
# Deliberately wider than equiv's generator. These inputs only decide whether a
# case is labelled correctly; they never feed equiv.

INT_CORNERS = sorted(
    {s * v + d for s in (1, -1) for v in (2**31, 2**32, 2**53, 2**63, 10**6, 10**18) for d in (-1, 0, 1)}
)
STR_POOL = (
    "abcxyzAZ09 _-.,"
    "\t\n\r\x0b\x0c\x1c\x1d\x1e\x1f\x85\xa0  　"
    "ßéİıﬀΣσς²³٣１①"
    "\U0001f600𐏿"
)
FLOAT_BITS = [
    0x0, 0x8000000000000000, 0x3FF0000000000000, 0xBFF0000000000000,
    0x7FF0000000000000, 0xFFF0000000000000, 0x7FF8000000000000, 0x7FF8000000000001,
    0x1, 0x000FFFFFFFFFFFFF, 0x7FEFFFFFFFFFFFFF, 0xFFEFFFFFFFFFFFFF, 0x0010000000000000,
]


def bits_to_float(b):
    return struct.unpack(">d", b.to_bytes(8, "big"))[0]


def rand_val(rng, ty):
    if ty == "int":
        r = rng.random()
        if r < 0.15:
            return rng.choice(INT_CORNERS)
        if r < 0.6:
            return rng.randint(-50, 50)
        if r < 0.9:
            return rng.randint(-10**6, 10**6)
        return rng.randint(-10**20, 10**20)
    if ty == "str":
        r = rng.random()
        if r < 0.2:
            return "".join(rng.choice("0123456789") for _ in range(rng.randint(1, 24)))
        if r < 0.3:
            return "".join(rng.choice("0123456789X") for _ in range(10))
        if r < 0.4:
            return rng.choice(["GB", "DE", "FR"]) + "".join(
                rng.choice("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ ") for _ in range(rng.randint(2, 30))
            )
        n = rng.choice([0, 1, 2, 3, 5, 8, 12, 40, 300])
        return "".join(rng.choice(STR_POOL) for _ in range(n))
    if ty in ("list[int]", "list"):
        n = rng.choice([0, 1, 2, 3, 4, 6, 10, 13, 20])
        if rng.random() < 0.4:
            return [rng.randint(0, 9) for _ in range(n)]
        return [rng.randint(-30, 30) for _ in range(n)]
    if ty == "float":
        r = rng.random()
        if r < 0.3:
            return bits_to_float(rng.choice(FLOAT_BITS))
        if r < 0.5:
            return bits_to_float(rng.getrandbits(64))
        if r < 0.7:
            return rng.randint(-2000, 2000) / 4
        return rng.uniform(-1e6, 1e6) * rng.choice([1, 1e-300, 1e300, 1e16])
    if ty in ("dict", "dict[str,int]"):
        keys = list("abcdefgh") + ["port", "x"]
        return {rng.choice(keys): rng.randint(-30, 30) for _ in range(rng.randint(0, 6))}
    raise ValueError(f"unknown arg type {ty}")


def exhaustive_inputs(types):
    """Deterministic sweeps for single-argument cases."""
    if types == ["int"]:
        for n in range(-5000, 5001):
            yield (n,)
        for n in INT_CORNERS:
            yield (n,)
    elif types == ["str"]:
        for cp in range(0x110000 if os.environ.get("DIVERGENCES_FULL_UNICODE") else 0x10000):
            yield (chr(cp),)
    elif types == ["float"]:
        for b in FLOAT_BITS:
            yield (bits_to_float(b),)


def integrity(case, n):
    d = case["dir"]
    try:
        ref = load_fn(d / "reference.py", case["function"], "ref")
        cand = load_fn(d / "candidate.py", case["function"], "cand")
    except Exception as e:  # noqa: BLE001
        return f"could not load: {type(e).__name__}: {e}"
    if case["expected"] == "diverges":
        w = parse_witness(case["witness"])
        r, c = outcome(ref, w), outcome(cand, w)
        if r == c:
            return f"witness does not diverge (both {r})"
        return None
    types = [t.strip() for t in case["args"].split(",") if t.strip()]
    rng = random.Random(case["id"])
    inputs = list(exhaustive_inputs(types))
    inputs += [tuple(rand_val(rng, t) for t in types) for _ in range(n)]
    for args in inputs:
        r, c = outcome(ref, args), outcome(cand, args)
        if r != c:
            return f"labelled equivalent but differs at {ascii(args)[:120]}: ref {r[:60]} / cand {c[:60]}"
    return None


# ---------------------------------------------------------------- scoring


def run_equiv(binary, case):
    d = case["dir"]
    env = dict(os.environ, EQUIV_METRICS="off")
    p = subprocess.run(
        [binary, "review", str(d / "candidate.py"), str(d / "reference.py"), case["function"], case["args"]],
        capture_output=True, text=True, env=env, cwd=ROOT,
    )
    lines = (p.stdout or p.stderr).splitlines()
    first = lines[0] if lines else ""
    detail = first
    if first.startswith("COUNTEREXAMPLE") and len(lines) >= 3:
        detail = f"{first} | {lines[1].strip()} | {lines[2].strip()}"
    m = re.search(r"agreed on all (\d+) generated inputs", first)
    return p.returncode, detail, int(m.group(1)) if m else None


def classify(expected, code):
    if expected == "diverges":
        return {1: "caught", 0: "missed"}.get(code, "could-not-check")
    return {0: "pass", 1: "false-alarm"}.get(code, "could-not-check")


GOOD = {"caught", "pass"}


def equiv_version():
    # The CLI has no --version flag; the workspace manifest is the source.
    cargo = (ROOT / "Cargo.toml").read_text()
    m = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M)
    return m.group(1) if m else "unknown"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--update-baseline", action="store_true")
    ap.add_argument("--skip-integrity", action="store_true")
    ap.add_argument("--integrity-n", type=int, default=10000)
    ap.add_argument("--only")
    a = ap.parse_args()

    cases = load_cases(a.only)
    failures = []

    if not a.skip_integrity:
        print(f"Corpus integrity ({len(cases)} cases)...")
        for c in cases:
            err = integrity(c, a.integrity_n)
            if err:
                failures.append(f"CORPUS ERROR {c['id']}: {err}")
                print(f"  CORPUS ERROR {c['id']}: {err}")
        print("  ok" if not failures else f"  {len(failures)} corpus error(s)")

    results = {}
    for c in cases:
        code, detail, n = run_equiv(a.bin, c)
        results[c["id"]] = dict(
            expected=c["expected"], cls=c["class"], exit=code, outcome=classify(c["expected"], code),
            n=n, detail=detail,
        )

    base = json.loads(BASELINE.read_text()) if BASELINE.exists() else None
    base_cases = (base or {}).get("cases", {})

    w = max(len(i) for i in results)
    print()
    print(f"{'case':<{w}}  {'expected':<10}  {'exit':>4}  {'outcome':<15}  baseline")
    print("-" * (w + 50))
    notes = []
    for cid, r in results.items():
        b = base_cases.get(cid)
        bmark = "new" if b is None else ("=" if b["outcome"] == r["outcome"] else b["outcome"] + " -> " + r["outcome"])
        print(f"{cid:<{w}}  {r['expected']:<10}  {r['exit']:>4}  {r['outcome']:<15}  {bmark}")
        if r["outcome"] == "false-alarm":
            failures.append(f"FALSE ALARM {cid}: {r['detail']}")
        if b is not None and b["outcome"] != r["outcome"]:
            if b["outcome"] in GOOD:
                failures.append(f"REGRESSION {cid}: {b['outcome']} -> {r['outcome']}")
            elif r["outcome"] in GOOD:
                notes.append(f"BETTER {cid}: {b['outcome']} -> {r['outcome']} (run with --update-baseline)")
            else:
                notes.append(f"CHANGED {cid}: {b['outcome']} -> {r['outcome']}")
    for cid in base_cases:
        if cid not in results and not a.only:
            notes.append(f"REMOVED {cid}: in baseline but not in corpus")

    div = [r for r in results.values() if r["expected"] == "diverges"]
    eqv = [r for r in results.values() if r["expected"] == "equivalent"]
    caught = sum(r["outcome"] == "caught" for r in div)
    missed = sum(r["outcome"] == "missed" for r in div)
    fa = sum(r["outcome"] == "false-alarm" for r in eqv)
    cnc = sum(r["outcome"] == "could-not-check" for r in results.values())

    def pct(x, y):
        return f"{100 * x / y:.1f}%" if y else "n/a"

    print()
    print("By class (diverges caught / diverges total; equivalent passed / equivalent total):")
    for cls in sorted({r["cls"] for r in results.values()}):
        rs = [r for r in results.values() if r["cls"] == cls]
        dv = [r for r in rs if r["expected"] == "diverges"]
        ev = [r for r in rs if r["expected"] == "equivalent"]
        print(f"  {cls:<20} diverges {sum(r['outcome'] == 'caught' for r in dv)}/{len(dv)}"
              f"   equivalent {sum(r['outcome'] == 'pass' for r in ev)}/{len(ev)}")
    print()
    print(f"Cases:            {len(results)} ({len(div)} diverges, {len(eqv)} equivalent)")
    print(f"Catch rate:       {caught}/{len(div)} = {pct(caught, len(div))} of diverges cases"
          f" ({missed} missed, {len(div) - caught - missed} could not check)")
    print(f"False-alarm rate: {fa}/{len(eqv)} = {pct(fa, len(eqv))} of equivalent cases")
    print(f"Could not check:  {cnc}")
    if missed:
        print()
        print("Missed (equiv reported EQUIVALENT on a diverging pair):")
        for cid, r in results.items():
            if r["outcome"] == "missed":
                print(f"  {cid}: {r['detail']}")
    for line in notes:
        print(line)

    if a.update_baseline:
        if a.only:
            sys.exit("--update-baseline needs the full corpus (drop --only)")
        out = {
            "equiv_version": equiv_version(),
            "python": platform.python_version(),
            "summary": {
                "cases": len(results), "diverges": len(div), "equivalent": len(eqv),
                "caught": caught, "missed": missed, "false_alarms": fa, "could_not_check": cnc,
            },
            "cases": {cid: {k: r[k] for k in ("expected", "exit", "outcome", "n", "detail")}
                      for cid, r in results.items()},
        }
        BASELINE.write_text(json.dumps(out, indent=2, ensure_ascii=True) + "\n")
        print(f"\nBaseline written to {BASELINE.relative_to(ROOT)}")
        failures = [f for f in failures if not f.startswith("REGRESSION")]
    elif base and base.get("python", "").rsplit(".", 1)[0] != platform.python_version().rsplit(".", 1)[0]:
        print(f"\nnote: baseline recorded on Python {base.get('python')}, running {platform.python_version()}")

    if failures:
        print()
        for f in failures:
            print(f)
        sys.exit(1)


if __name__ == "__main__":
    main()
