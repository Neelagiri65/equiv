# Case study: jellyfish, Rust against its own Python twin

[jellyfish](https://github.com/jamesturk/jellyfish) is a widely used Python
library for fuzzy string matching (Soundex, Metaphone, Levenshtein distance and
others). Each function ships twice in the same package: a fast Rust version
(`jellyfish._rustyfish`, the one you normally get) and a pure-Python version
(`jellyfish._jellyfish`). The two are meant to give the same answers.

That makes it a natural test for `equiv`: treat the Python version as the
reference and the Rust version as the change, and ask whether they behave the
same.

We ran this offline against the published wheels, in a container with no
network. Two versions: **1.2.1**, which shipped a Metaphone bug later fixed by
[pull request #247](https://github.com/jamesturk/jellyfish/pull/247), and **1.3.0**,
which includes the fix. The maintainer was not contacted and no issues were filed.

Everything below was produced by `equiv` v0.14.0 and can be re-run:
[`examples/case-studies/jellyfish/`](../../examples/case-studies/jellyfish/).

## Finding 1: the two copies disagree in 8 of 10 functions

Each function was checked once with the default settings (seed 1). Rust is the
candidate, Python the reference. The results were the same on 1.2.1 and 1.3.0
except for the Metaphone count, which is covered in Finding 2.

| function | verdict | inputs that disagree | first disagreement |
|---|---|---|---|
| `soundex` | equivalent | 0 of 1,589 | |
| `nysiis` | equivalent | 0 of 1,589 | |
| `metaphone` | diverges | 12 (1.2.1), 6 (1.3.0) of 1,589 | `'Straße'`: Rust `STRS`, Python `STR` |
| `match_rating_codex` | diverges | 3 of 1,589 | `''`: Rust `''`, Python raises `ValueError` |
| `jaro_similarity` | diverges | 1 of 2,086 | `('é', 'É')` written as e + combining accent |
| `jaro_winkler_similarity` | diverges | 1 of 2,086 | same input |
| `levenshtein_distance` | diverges | 5 of 2,086 | `('\r\n', '')`: Rust 1, Python 2 |
| `damerau_levenshtein_distance` | diverges | 5 of 2,086 | same input |
| `hamming_distance` | diverges | 5 of 2,086 | same input |
| `match_rating_comparison` | diverges | 476 of 2,086 | `('', '')`: Rust `True`, Python raises `ValueError` |

"Inputs" are the distinct generated inputs that `equiv` judged: built-in corner
cases plus random strings. Receipt ids for every row are in
[`out-1.2.1.txt`](../../examples/case-studies/jellyfish/out-1.2.1.txt) and
[`out-1.3.0.txt`](../../examples/case-studies/jellyfish/out-1.3.0.txt).

Many of these disagreements look deliberate rather than wrong. The Rust
distance functions count user-perceived characters (`\r\n` is one), the Python
ones count code points. Rust accepts empty strings where Python raises. `equiv`
reports that the behaviour differs. It does not say which side is right; only
the maintainer can.

### On plain ASCII words too

To rule out accents and control characters as the only cause, we wrapped both
sides in the same guard (ASCII letters and spaces only) and re-ran Metaphone and
the match-rating codex on three seeds (1, 3, 5). They still disagree:

- **Metaphone**, words ending in `mb` or containing `gn`: `'mb'` gives Rust `M`,
  Python `MB`; `'ngn'` gives Rust `N`, Python `NKN`. Present in 1.3.0.
- **Match-rating codex**, strings with spaces: `' x '` gives Rust `' X '`,
  Python `'X'`. Present in 1.3.0.

A hand check (not an `equiv` result) shows what the `mb` case means for a real
word: `metaphone('dumb')` is `TM` from Rust and `TMB` from Python. The original
Metaphone rules drop a B after M at the end of a word, so here the Rust version
follows the rules and the Python one does not. We have not reported this.

## Finding 2: it would have caught the #247 bug before it shipped

In 1.2.1, Rust Metaphone turned a word-final "gh" into `KH`: `'high'` gave
`HKH` instead of `H`. Pull request #247 fixed it in 1.3.0.

We counted how many of the Metaphone disagreements `equiv` listed (with
`--all`) were words ending in "gh", for different input counts and five seeds:

| inputs (n) | 1.2.1, seeds 1 / 3 / 5 / 7 / 9 | 1.3.0, all five seeds |
|---|---|---|
| 200 | 0 / 0 / 0 / 0 / 0 | 0 |
| 500 | 2 / 0 / 1 / 0 / 0 | 0 |
| 1,000 | 5 / 1 / 2 / 0 / 1 | 0 |
| 2,000 (the default since v0.14.0) | 6 / 1 / 6 / 3 / 4 | 0 |

At the default of 2,000 inputs, every seed tried on 1.2.1 surfaced the "gh" bug,
with inputs such as `'gh'` (Rust `KH`, Python empty) and `'rczgh'` (Rust `RKSKH`,
Python `RKS`). On 1.3.0, after the fix, none did. At 200 inputs, the default
before v0.14.0, no seed found it.

Two changes made this possible, and both shipped in `equiv` this month:
v0.13.0 counts every disagreeing input and lists them with `--all` (before, it
stopped at the first, which for Metaphone is `'Straße'`), and v0.14.0 raised the
default from 200 to 2,000 inputs.

## Limits

- **Generated inputs only.** A green result means the two versions agreed on
  the inputs `equiv` generated, not on every possible string. Soundex and NYSIIS
  being equivalent here is evidence, not proof.
- **The bug was found with hindsight.** We knew #247 existed and counted how
  often `equiv` reached it. In a real review, the "gh" lines would have been
  among 12 Metaphone disagreements, next to the deliberate-looking ones; a person
  still has to read them.
- **Seed-dependent at smaller sizes.** Below 2,000 inputs, whether the bug shows
  up depends on the seed. Five seeds is a small sample.
- **Disagreement is not a bug.** Only #247 is a confirmed bug. The `mb`/`gn`
  and spacing differences are unreported disagreements; most of the rest look
  intentional.
- **Platform.** The receipt ids were produced on linux-arm64 with the pinned
  `python:3.12` image in `run.sh`.
