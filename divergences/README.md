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

Recorded with equiv 0.2.1 on Python 3.14 (see `BASELINE.json`).

| | |
|---|---|
| Cases | 60 (40 diverges, 20 equivalent) |
| Catch rate | 25 / 40 = 62.5% |
| False-alarm rate | 0 / 20 = 0% |
| Could not check | 1 |

By class (diverges caught / total, equivalent passed / total):

| class | diverges | equivalent |
|-------|----------|------------|
| int-to-float | 2/2 | - |
| or-default | 3/4 | 1/1 |
| float-specials | 3/3 | 2/2 |
| float-rounding | 2/4 | - |
| exception-vs-value | 3/3 | - |
| unicode | 0/5 | 3/3 |
| fixed-width-int | 1/3 | 3/3 |
| negative-division | 3/3 | 2/2 |
| boundary | 2/2 | 4/4 |
| ordering | 3/3 | 2/2 |
| check-digit | 2/5 | 3/3 |
| rare-input | 1/3 | - |

### Misses and why

Each of these is a pair that diverges on its recorded witness and that equiv
reported as equivalent. The reason is the generator's reach, read from
`crates/equiv-review/src/lib.rs`:

| case | why equiv did not reach the witness |
|------|-------------------------------------|
| 006-or-default-dict-rare-key | dict inputs use keys `a`-`h` only; the `"port"` key is never generated |
| 012-float-reassociation | needs operands that cancel (e.g. `1e16, -1e16, 1.0`); the random three-float draws did not produce one |
| 017-unicode-lower-vs-casefold | string inputs are ASCII; `ß` is never generated |
| 018-unicode-isdigit-vs-isdecimal | needs a non-ASCII digit such as `²` |
| 019-unicode-strip-variants | the only whitespace generated is a single space; no tab or newline |
| 020-unicode-byte-vs-char-length | for ASCII strings UTF-8 byte length equals character length |
| 021-unicode-lone-surrogate | lone surrogates are never generated |
| 023-fixedwidth-int32-wrap-large | ints stay within +/-1,000,000, below 2^31 |
| 024-int-float-precision-large | ints stay within +/-1,000,000, far below 2^53 |
| 030-boundary-long-string-truncate | generated strings are at most 10 characters; the limit is 255 |
| 034-checkdigit-luhn-str-mod9 | no multi-digit strings are generated, so both validators reject every input |
| 035-checkdigit-luhn-list-parity | random lists draw from -20..20, so all-digit lists are rare; the fixed corner lists happen to agree |
| 036-checkdigit-isbn10-x | needs a 10-character ISBN-shaped string |
| 040-rare-magic-computed | the magic value (`1 << 19`) is not a literal, so the source scan does not target it |

Could not check: `011-float-round-half-even-floor` is refused because
`math.floor` is outside the float allowlist (only `+ - * /`, comparisons, a few
exact builtins and `math.sqrt` are admitted). Its sibling
`010-float-round-half-even-int` (same bug, written with `int(x + 0.5)`) is
caught.

Contrast pairs worth reading together: `039-rare-magic-literal` (caught, because
the magic amount is a literal and the source scan aims inputs at it) and
`040-rare-magic-computed` (missed, same bug, value computed);
`037-checkdigit-iban-letter-map` and `038-checkdigit-ean-ten-vs-zero` (caught)
against `034`-`036` (missed).

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
