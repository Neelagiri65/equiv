# Divergence corpus

A public, reproducible benchmark for `equiv review`: pairs of Python functions
where we know the right answer, so we can measure how often equiv catches a real
behaviour change and how often it raises a false alarm on a refactor that keeps
behaviour. Think test262, but for a behavioural-equivalence checker.

Each case is a directory:

| file | contents |
|------|----------|
| `reference.py` | the original function |
| `candidate.py` | the rewrite |
| `case.toml` | `function`, `args` (equiv arg types), `expected` (`"diverges"` or `"equivalent"`), `class`, a one-line `pattern`, optional `source` (URL of real-world inspiration or the documented behaviour), for `diverges` cases a `witness`, and optional `integrity_max_abs_int` (skips larger ints in the plain-Python label check, for references that loop \|n\| times) |

`witness` is a Python tuple literal of arguments, stored as a TOML literal string
so NaN, infinities, `-0.0` and lone surrogates can be written, for example
`witness = '''(float('nan'),)'''` or `witness = '''('\ud800',)'''`.

## Scope

The corpus measures **behavioural equivalence on generated inputs only**: the
same thing equiv measures. An `equivalent` verdict from equiv means "the two
versions agreed on N generated inputs", nothing more. It does not mean the
rewrite is correct, that it matches intent, or that no diverging input exists;
the misses below are concrete pairs where equiv says `EQUIVALENT` and the
versions differ. The corpus says nothing about intent, architecture, security or
whole-program behaviour.

## Running it

```
scripts/run-divergences.sh                    # build, check, score, compare to baseline
scripts/run-divergences.sh --only checkdigit  # a subset (directory-name substring)
scripts/run-divergences.sh --update-baseline  # rewrite BASELINE.json from this run
EQUIV_BIN=/path/to/equiv scripts/run-divergences.sh   # score an existing binary
```

Requires Rust (to build the CLI) and Python 3.11+ (`tomllib`); equiv itself
already needs `python3`.

The runner does two passes.

1. **Corpus integrity**, independent of equiv. Every `diverges` witness is run
   through both versions and must give different canonical results (the same
   canonical form equiv uses: `42` and `42.0` differ, `True` and `1` differ, all
   NaNs are one value, dicts compare as maps, a raised exception compares by type
   name). Every `equivalent` pair is compared on 10,000 randomised inputs drawn
   from a wider distribution than equiv's generator; single-argument `int`,
   `str` and `float` pairs also get a deterministic sweep (all ints in
   [-5000, 5000] plus 32/53/64-bit neighbours, every BMP code point as a
   one-character string, the float corner bit patterns). The random inputs
   cover what equiv's generator does not reach (Unicode,
   whitespace variants, surrogates, long and digit strings, ints up to 10^20,
   random float bit patterns). A case that fails either check is mislabelled and
   the run fails with a `CORPUS ERROR`.
2. **Scoring.** `equiv review candidate.py reference.py <function> <args>` runs
   on every case with the default case count and seed.

## Scoring rules

| expected | exit 1 (counterexample) | exit 0 (equivalent) | exit 2 (could not check) |
|----------|-------------------------|---------------------|--------------------------|
| `diverges` | **caught** | **missed** | could-not-check |
| `equivalent` | **false alarm** | pass | could-not-check |

- **Catch rate** = caught / diverges cases.
- **False-alarm rate** = false alarms / equivalent cases.
- **Could not check** (refused by the float allowlist, a non-JSON-structural
  value, a load error) is counted separately: it is neither a catch nor a pass.

The runner exits non-zero only on:

- any false alarm;
- any corpus error;
- a regression against `BASELINE.json`: a case that was `caught` or `pass` in
  the baseline and is now anything else;
- a case that is not caught (or passed) and has no `writeoff`, or a `writeoff`
  on a case that is now caught (see the completeness gate below).

Misses and could-not-check results are what the corpus exists to measure; the
gate only requires that each one has its reason recorded. A case that moves to
a better outcome is printed as `BETTER` with a prompt to refresh the baseline.

`BASELINE.json` records, per case, the exit code, outcome, number of generated
inputs and the first line of equiv's verdict, plus the equiv and Python versions
it was recorded with. String methods follow the host's Unicode tables, so a
different Python minor version is flagged in the output.

## Current results

Recorded with equiv 0.28.0 on Python 3.14 (see `BASELINE.json`).

| | |
|---|---|
| Cases | 131 (88 diverges, 43 equivalent) |
| Catch rate | 81 / 88 = 92.0% |
| False-alarm rate | 0 / 43 = 0% |
| Could not check | 4 |

Cases 116-131 (class `holdout`) were written on 2026-11-12, before any
generator code changed, and recorded against 0.27.0 first: 0.27.0 catches
0 / 12 of their diverging pairs and passes 4 / 4 of their equivalent pairs.
They test four changes in 0.28.0, each with a correct rewrite that must stay
equivalent and one case written off the planned input list on purpose:

- list elements and dict values aimed at the source's literals and drawn from
  wider ranges (116, 117 caught; 118 equivalent; 119 off the list: the
  threshold is `ord("d")`, not a number in the source, and stays missed);
- numeric literals read with both signs, int literals and exponent forms
  offered to float arguments, each with the next float below and above (120,
  121, 122 caught; 123 equivalent; 124 off the list: the threshold is
  `0.1 + 0.2`, the sum of two literals, and stays missed);
- string literals decoded as Python decodes them, with the whole prefix read
  (125 octal escape and 128 `Rf` raw prefix caught; 127 equivalent; 126 off
  the list: a `\N{...}` escape, which is skipped, not guessed, and stays
  missed);
- cross cases, each argument's corners and literals paired with a few
  non-degenerate values of the others (129 caught at `('\U0001f600', 2)`;
  130 equivalent; 131 off the list: it needs the 16-digit corner with the int
  16, which is not one of the fixed partner values, and stays missed).

0.28.0 catches 8 / 12, with 0 / 4 false alarms; the four off-list cases are
missed and written off below with their reasons. No earlier case changed
outcome. The four off-list misses are what the off-list cases are for: aiming
at literals does not generalise to constants that are computed, and the cross
cases do not generalise beyond their fixed partner values.

Cases 108-111 (class `holdout`) were written on 2026-11-06, before the
int()-grammar string corners, to test that class rather than the witnesses of
074 and 076: separators via `split` + `strip` (108), a hand-written signed-int
parser (109), underscore digit groups (110) and an equivalent redundant strip
(111). Recorded on 0.20.0 before the corners: 108 and 109 missed, 110 caught
by the random mixed alphabet, 111 passes. The corners (U+001C-U+001F padding,
the only characters `str.strip()` removes that `int()` and `float()` reject;
sign runs; PEP 515 underscores) then catch 074, 076, 108 and 109, with 0 / 37
false alarms. This reverses DECISIONS #5 ("doubled signs / rare invisible
separators: not now") under path A: the corners come from int()'s own grammar,
not from a case witness.

Cases 104-107 (class `holdout`, "mixed") were written on 2026-11-05, before
any generator change, from a run against uutils `sort` where equiv found
nothing: they need uppercase, digits or control characters inside a string,
or two such strings in one list. All four are missed by 0.16.0. 0.17.0 widens
random strings (mixed case, digits, control characters, numeric text) and adds
near-duplicate list elements; it catches all four on seed 1, and 104, 105 and
107 on each of seeds 2, 3, 5, 7 and 11 (106 on 3 of those 5).

History: equiv 0.2.1 caught 25 / 40 of the original 60 cases. The 0.3.0
generator catches 36 / 40 of them. Cases 061-070 (class `holdout`) were
written *after* the 0.3.0 generator change, to check the improvement is not
fitted to the original misses: 0.2.1 catches 2 / 6 of their diverging pairs,
0.3.0 catches 5 / 6, with 0 / 4 false alarms.

Cases 071-078 (also `holdout`) were written *before* the 0.5.0 numeric-text
string corners (`-1`, `+1`, `1.5`, `1e3`, ` 7 `, `-0`, `+0`, `0.0`) were
added, and recorded against 0.4.0 first: 0.4.0 catches 0 / 5 of their diverging
pairs, 0.5.0 catches 3 / 5, with 0 / 3 false alarms. 064 is caught by the same
change. The two still missed sit next to the new corners, not on them: 076 (a
repeated sign) was written that way to test whether the corners generalise, and
they do not; 074 began as an equivalent pair and was relabelled before equiv
ran on it, when a check of the code showed `str.strip()` removes the
U+001C-U+001F separators that `int()` rejects.

Cases 079-082 (also `holdout`) were written *before* the 0.6.0 joint numeric
corners (for every pair of same-type int or float arguments: `(x, -x)`,
`(-x, x)`, `(x, x)` with `x` in 1, 2, 1,000,000 for ints and 2^54, the largest
finite float for floats, plus `(+inf, -inf)` both ways) and recorded against
0.5.0 first: 0.5.0 catches 2 / 3 of their diverging pairs, 0.6.0 catches 3 / 3,
with 0 / 1 false alarms. 012 is caught by the same change. 080 (product
reassociation, witness `MAX, 2.0, 0.5`) was written off the planned corner list
on purpose and was already caught by the random draws on 0.5.0, so it measures
nothing about the new corners either way.

Cases 083-085 (also `holdout`) were written *before* the 0.7.0 large-magnitude
int corners (2^31 - 1, 2^31, 2^31 + 1, 2^32, 2^49 + 1, 2^53 - 1, 2^53,
2^53 + 1 and the i64 limits, each with its negative, run after every other
input) and recorded against 0.6.0 first: 0.6.0 catches 0 / 2 of their
diverging pairs, 0.7.0 catches 1 / 2, with 0 / 1 false alarms. 024 is caught
by the same change. 085 (digit count via `math.log10`, witness 10^18 - 1) was
written off the planned power-of-two list on purpose, to test whether the
corners generalise; they do not reach it. A function whose work grows with
`n` cannot finish at these corners inside the step budget; from 0.7.0 such a
corner is recorded in the receipt as not judged and the review goes on, so
no corpus case changed outcome for that reason (no equivalent case carries
an unjudged count).

Cases 086-088 (also `holdout`) were written *before* the 0.8.0 admissibility
guard on every signature (a function that reaches a maths-library result is
refused by name whatever its argument types; exact functions such as `gcd`,
`isqrt` and `fabs` stay admitted) and recorded against 0.7.0 first: 0.7.0
judges both libm pairs and catches 1 / 2 (088 at `[0]`, an exception
difference); 0.8.0 refuses both, so they count as could-not-check, and the
exact pair 087 passes on both (0 / 1 false alarms). 085 is refused by the same
change. A refusal is not a miss and not a catch: the receipt says the function
was not judged, and why.

Cases 089-092 (also `holdout`) were written *before* the 0.10.0 `bool` and
`list[str]` signatures and recorded against 0.9.0 first, where the CLI rejects
both signatures (exit 3, could-not-check on all four). 0.10.0 catches 2 / 3 of
their diverging pairs, with 0 / 1 false alarms. 092 (`n * 2` vs `int(n * 2.0)`
behind a bool flag, witness `(2^53 + 1, True)`) was written off the planned
list on purpose: its divergence needs the large-int corner AND the flag at its
non-default value, and corners are enumerated one argument at a time with the
others at their default (`False`), so it is missed. That is a finding about
enumeration, recorded in the misses table, not a reason to add a corner family
in the same change.

Cases 093-095 (also `holdout`) were written *before* the 0.11.0 bool fan-out
(every corner block also runs once per bool argument with that flag set to
`True`) and recorded against 0.10.0 first: 0.10.0 catches 0 / 2 of their
diverging pairs, 0.11.0 catches 1 / 2, with 0 / 1 false alarms. 092 is caught
by the same change, at `(2^53 + 1, True)`. 095 (a large-int divergence that
needs two flags set at once) was written off the planned list on purpose: the
fan-out sets one flag at a time, so it was missed. 0.20.0 adds one pass with
every flag `True` when a signature has two or more bools (one extra pass
whatever the number of bools; with two bools the bool space is covered
completely); 095 is caught at `(9007199254740993, True, True)`. A divergence
that needs a strict subset of three or more flags is still out of reach.

By class (diverges caught / total, equivalent passed / total):

| class | diverges | equivalent |
|-------|----------|------------|
| int-to-float | 2/2 | - |
| or-default | 4/4 | 1/1 |
| float-specials | 3/3 | 2/2 |
| float-rounding | 4/4 | - |
| exception-vs-value | 3/3 | - |
| unicode | 5/5 | 3/3 |
| fixed-width-int | 3/3 | 3/3 |
| negative-division | 3/3 | 2/2 |
| boundary | 2/2 | 4/4 |
| ordering | 3/3 | 2/2 |
| check-digit | 5/5 | 3/3 |
| rare-input | 3/3 | - |
| holdout | 41/48 | 22/23 |

### Misses, write-offs and the completeness gate

There were no misses from 0.20.0 to 0.27.0. As of 0.28.0 there are four
(119, 124, 126, 131: equiv says `EQUIVALENT` and the versions differ), all
written off the planned input list on purpose and listed below. Every case equiv does not catch (or, if
equivalent, does not pass) carries a `writeoff` in its `case.toml` with the
reason it is out of reach by design. The runner fails on a case that is not
caught and has no write-off (`OPEN`), and on a write-off left on a case that is
now caught (`STALE`). This is path A's completeness gate: inside the fixed
scope, everything is caught or written off with its reason.

| case | outcome | why it is out of reach by design |
|------|---------|----------------------------------|
| 085-holdout-digit-count-via-log10 | could not check | reaches `math.log10`, whose last bit is not the same on every host (DECISIONS #17) |
| 086-holdout-digit-count-via-log-base | could not check | reaches `math.log` (same reason) |
| 088-holdout-list-bit-length-via-log2 | could not check | reaches `math.log2` (same reason) |
| 099-holdout-four-line-loop-body-scope-edge | could not check | the reference passes the default step budget at input 1000002; a run past the budget is not judged (DECISIONS #16, #22) |
| 119-holdout-list-element-computed-threshold | missed | the threshold is computed at import time (`ord` of a character) and is not a number in the source; the literal scan reads tokens and does not evaluate |
| 124-holdout-float-computed-threshold | missed | the threshold is the sum of two float literals; the scan folds only int literal-op-literal |
| 126-holdout-named-escape-literal | missed | the character is written as `\N{...}`; equiv has no Unicode name table, and a literal it cannot decode exactly is skipped, never guessed |
| 131-holdout-str-int-corner-with-unlisted-int | missed | needs a string corner paired with an int that is neither a fixed partner value (1, 2) nor a literal; cross cases are a capped set, not a cartesian product |

Closed in 0.20.0 (path A step 3):

- `011-float-round-half-even-floor` was refused because `math.floor` was
  outside the float allowlist. 0.20.0 admits the exact math functions
  (`floor`, `ceil`, `trunc`, `fabs`, `copysign`, `fmod`, `modf`, `frexp`,
  `ldexp`, `isfinite`, `isnan`, `isinf`: results exactly representable, so the
  same bits on every host) and adds exact halves (0.5, -0.5, 1.5, 2.5, -2.5) to
  the float corners; 011 is caught at `(0.5)`.
- `021-unicode-lone-surrogate`: a lone surrogate cannot be held in a Rust
  `String`. 0.20.0 carries three surrogate corners as UTF-16 code units (high,
  low, and a low surrogate after text); 021 is caught at `('\ud800')`.
  Holdouts 112 (diverges) and 113 (equivalent) were written first: 112 missed
  before the change, caught after; 113 passes throughout.
- `074` and `076`: see cases 108-111 above.
- `095`: see the bool fan-out paragraph above.

Closed in 0.21.0 (path A step 5):

- A Rust panic surfacing through PyO3 (`pyo3_runtime.PanicException`) derives
  from `BaseException`, so equiv stopped the run and reported *could not
  check* (path A step 2: pendulum 3.3.0 `week_day`). It is now observed like
  an exception: a panic on one side and a value on the other is a
  counterexample; the same panic on both sides is agreement. Other
  non-`Exception` exits (`SystemExit`, `KeyboardInterrupt`) still stop the run.
  Holdouts 114 (diverges) and 115 (equivalent) were written first; both were
  *could not check* on 0.20.0; 114 is caught and 115 passes on 0.21.0.

Contrast pair worth reading together: `023-fixedwidth-int32-wrap-large` is now
caught because its masks (`0xFFFFFFFF`, `0x80000000`) are in the source and the
literal scan aims at them; `024-int-float-precision-large` has no such constant
and is caught by the large-int corners (at `(9007199254740993)`).

## Adding a case

1. Create `divergences/NNN-short-slug/` with the next free number.
2. Write `reference.py` and `candidate.py`, each defining the function at module
   level. Use only equiv's arg types (`int`, `str`, `list[int]`, `float`,
   `dict`) and return JSON-structural values (no tuples or sets).
3. Keep the witness, and values next to it, **out of both source files**,
   including comments: equiv scans the source for literals and aims inputs at
   them, so a stray literal turns a generator-reach case into a source-scan case.
   If that is what you want to test, say so in `pattern`.
4. Do not add an `__init__.py`; equiv would then load the files as a package.
5. Write `case.toml`:

   ```toml
   function = "is_adult"
   args = "int"
   expected = "diverges"
   class = "boundary"
   pattern = ">= threshold rewritten as >, excluding the boundary value"
   source = "https://..."          # optional
   witness = '''(18,)'''           # required for diverges
   # writeoff = "..."              # only if equiv cannot reach it by design: the reason
   ```

6. Run `scripts/run-divergences.sh`. The integrity pass must be clean: your
   witness must diverge, or your equivalent pair must agree on the independent
   inputs. Then run with `--update-baseline` and commit the new baseline entry
   with the case.

Label by what the code does, never by what equiv reports. A case equiv misses
stays in the corpus as a miss; do not relabel or drop it to raise the score,
and do not change equiv's engine or generator to fit a particular case.
