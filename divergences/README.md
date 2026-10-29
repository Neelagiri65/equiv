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
| `case.toml` | `function`, `args` (equiv arg types), `expected` (`"diverges"` or `"equivalent"`), `class`, a one-line `pattern`, optional `source` (URL of real-world inspiration or the documented behaviour), and for `diverges` cases a `witness` |

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
  the baseline and is now anything else.

Misses and could-not-check results never fail the run; they are what the corpus
exists to measure. A case that moves to a better outcome is printed as `BETTER`
with a prompt to refresh the baseline.

`BASELINE.json` records, per case, the exit code, outcome, number of generated
inputs and the first line of equiv's verdict, plus the equiv and Python versions
it was recorded with. String methods follow the host's Unicode tables, so a
different Python minor version is flagged in the output.

## Current results

Recorded with equiv 0.5.0 on Python 3.14 (see `BASELINE.json`).

| | |
|---|---|
| Cases | 78 (51 diverges, 27 equivalent) |
| Catch rate | 45 / 51 = 88.2% |
| False-alarm rate | 0 / 27 = 0% |
| Could not check | 1 |

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

By class (diverges caught / total, equivalent passed / total):

| class | diverges | equivalent |
|-------|----------|------------|
| int-to-float | 2/2 | - |
| or-default | 4/4 | 1/1 |
| float-specials | 3/3 | 2/2 |
| float-rounding | 2/4 | - |
| exception-vs-value | 3/3 | - |
| unicode | 4/5 | 3/3 |
| fixed-width-int | 2/3 | 3/3 |
| negative-division | 3/3 | 2/2 |
| boundary | 2/2 | 4/4 |
| ordering | 3/3 | 2/2 |
| check-digit | 5/5 | 3/3 |
| rare-input | 3/3 | - |
| holdout | 9/11 | 7/7 |

### Misses and why

Each of these is a pair that diverges on its recorded witness and that equiv
reported as equivalent. The reason is the generator's reach, read from
`crates/equiv-review/src/lib.rs`:

| case | why equiv did not reach the witness |
|------|-------------------------------------|
| 012-float-reassociation | needs all three operands set together so they cancel (e.g. `1e16, -1e16, 1.0`); corner floats are varied one argument at a time and the random draws did not produce one |
| 021-unicode-lone-surrogate | a lone surrogate cannot be held in a Rust `String`, so the generator cannot produce one |
| 024-int-float-precision-large | needs an int above 2^53 with no such constant in the source; random ints stay within +/-1,000,000 because a reference may loop `n` times and there is no execution time limit yet |
| 074-holdout-int-strip-separators | needs digits wrapped in U+001C-U+001F, which `str.strip()` removes and `int()` rejects; no string corner contains those characters |
| 076-holdout-is-int-double-sign | needs a repeated sign such as `--1`; the sign corners carry a single sign, and the source only spells `+-`, on which both sides agree |

Could not check: `011-float-round-half-even-floor` is refused because
`math.floor` is outside the float allowlist (only `+ - * /`, comparisons, a few
exact builtins and `math.sqrt` are admitted). Its sibling
`010-float-round-half-even-int` (same bug, written with `int(x + 0.5)`) is
caught.

Contrast pair worth reading together: `023-fixedwidth-int32-wrap-large` is now
caught because its masks (`0xFFFFFFFF`, `0x80000000`) are in the source and the
literal scan aims at them; `024-int-float-precision-large` has no such constant
and is still missed.

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
   ```

6. Run `scripts/run-divergences.sh`. The integrity pass must be clean: your
   witness must diverge, or your equivalent pair must agree on the independent
   inputs. Then run with `--update-baseline` and commit the new baseline entry
   with the case.

Label by what the code does, never by what equiv reports. A case equiv misses
stays in the corpus as a miss; do not relabel or drop it to raise the score,
and do not change equiv's engine or generator to fit a particular case.
