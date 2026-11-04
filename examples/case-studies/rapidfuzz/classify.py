"""Sort every disagreement in a run output into: type only (Python int vs an
equal C++ float, e.g. 0 vs 0.0, which == treats as equal), last-bits float
(< 1e-12 apart), or value (anything else). Usage: python3 classify.py out-3.14.6.txt"""
import re, sys
from collections import Counter, defaultdict

section, fn = "", None
kinds = defaultdict(Counter)
examples = {}
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    if line.startswith("## "):
        section = line[3:5].strip(".")
    elif line.startswith("== "):
        fn = f"{section} {line[3:].split()[0]}"
    elif line.startswith("  at ") and fn:
        m = re.search(r"^  at (.*): candidate -> (.*), reference -> (.*)$", line.rstrip("\n"))
        arg, c, r = m.groups()
        def num(x):
            try:
                return float.fromhex(x) if x.lstrip("-").startswith("0x") else (int(x) if re.fullmatch(r"-?\d+", x) else None)
            except ValueError:
                return None
        cv, rv = num(c), num(r)
        if cv is not None and rv is not None and isinstance(rv, int) != isinstance(cv, int) and cv == rv:
            k = "type only"
        elif isinstance(cv, float) and isinstance(rv, float) and abs(cv - rv) < 1e-12:
            k = "last bits"
        else:
            k = "value"
        kinds[fn][k] += 1
        examples.setdefault((fn, k), f"{arg}: C++ {c}, Python {r}")
total = Counter()
for fn, c in kinds.items():
    total.update(c)
    print(f"{fn:45} " + ", ".join(f"{k} {n}" for k, n in sorted(c.items())))
print("TOTAL", dict(total))
for (fn, k), ex in examples.items():
    if k == "value":
        print(f"value example  {fn}: {ex}")
