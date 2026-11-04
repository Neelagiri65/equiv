# Case study: rapidfuzz, C++ against its own Python fallback

[rapidfuzz](https://github.com/rapidfuzz/RapidFuzz) is a fuzzy string matching
library with about 116 million downloads a month (pypistats, November 2026).
Most of its functions ship twice in the same wheel: a compiled C++ version
(`rapidfuzz.fuzz_cpp`, `rapidfuzz.distance.metrics_cpp`, `rapidfuzz.utils_cpp`)
and a pure-Python fallback (`fuzz_py`, `metrics_py`, `utils_py`). The project
treats a difference between the two as a bug: in
[#353](https://github.com/rapidfuzz/RapidFuzz/issues/353) the maintainer called
one "a bug in the pure python fallback implementation" and fixed it.

We asked `equiv` whether the two behave the same, with C++ as the candidate and
Python as the reference. Everything ran offline against the published 3.14.6
wheel. The maintainer was not contacted and no issues were filed.

Produced by `equiv` v0.15.0 and re-runnable:
[`examples/case-studies/rapidfuzz/`](../../examples/case-studies/rapidfuzz/).

## Result

51 functions were checked. All but one take two strings and return a number;
`default_process` takes one string and returns a string.

| outcome | functions | detail |
|---|---|---|
| Same on every input | 26 | e.g. all integer distances and similarities, `ratio` |
| Differ only in type | 12 | Python returns `0`, C++ returns `0.0`; equal once both are wrapped in `float()` |
| Differ in the last bits of a float | 6 | e.g. `0.4555555555555555` (C++) vs `0.45555555555555555` (Python) |
| Differ in value | 7 | U+00A0 (6 token functions) and `İ` (`default_process`) |

The first run checked 49 functions as returned (26 same, 23 differ). The second
wrapped 34 numeric functions in `float()` on both sides, adding two that the
first run did not cover, and ran each on seeds 1, 3 and 5 (22 same on every
seed, 12 differ on every seed).

Across all 151 runs, `equiv` listed 12,782 disagreeing inputs. A script in the
example folder (`classify.py`) sorts every one of them: 10,463 are type only,
2,281 are last-bit float differences, and 38 are different values. All 38
involve U+00A0 or `İ`.

### Type only

For inputs where nothing matches, the Python fallback returns the integer `0`
and C++ returns the float `0.0`. Python's `==` treats these as equal, so most
callers will never notice. `equiv` compares types strictly, so it reports them.
To see past this, the second run wraps both sides in `float()`.

### Last bits

Six normalised scores differ by one or two units in the last place, for example
`jaro_normalized_similarity('qsqrcc', 'wbpgr')`. About 1 to 11% of inputs are
affected, depending on the function. `jaro_similarity` itself agrees; only the
normalised variant drifts. This matters only to code that compares scores with
`==` or uses them as dictionary keys.

### Different values: the non-breaking space

The six token-based scorers (`token_sort_ratio`, `token_set_ratio`,
`token_ratio` and their `partial_` versions) disagree whenever U+00A0, the
non-breaking space, is involved:

| input | C++ | Python |
|---|---|---|
| `token_sort_ratio('\xa0', '')` | 0.0 | 100.0 |
| `token_set_ratio('\xa0', '\xa0')` | 100.0 | 0 |

A hand check (not an `equiv` result) shows what this means for real text, where
non-breaking spaces are common in scraped HTML:

| input | C++ | Python |
|---|---|---|
| `token_sort_ratio('new york', 'york<U+00A0>new')` | 50.0 | 100.0 |

Python's `str.split()` treats U+00A0 as whitespace, so the fallback sees the
words "york" and "new" and scores a match. The C++ version does not split on
it. A further hand check found that C++ splits correctly on other Unicode
spaces (U+2003, U+3000, U+2009, U+202F, U+1680) but not on U+00A0 or U+0085,
the two in the Latin-1 range. We have not read the C++ source to confirm the
cause. We found no upstream issue about it.

`equiv` found this because U+00A0 is one of its fixed corner-case inputs
("whitespace other than a single space"), not by random chance. It shows up
identically on every seed.

### Different values: `İ`

`default_process('İ')` (capital I with dot above) returns `'i'` from C++ and
`'i'` followed by a combining dot (U+0307) from Python, which is what
`str.lower()` gives. `İ` is also a fixed corner case.

## Which side is right?

`equiv` does not say. For the non-breaking space, the Python fallback's
behaviour is arguably the more useful one, and it is the one most Python users
would expect from "token" functions. For `İ` there is no single right answer.
Only the maintainer can say which behaviour is intended. Given #353, a
difference between the two implementations is at least worth a look.

## Limits

- **Generated inputs only.** The functions that agree were tested on about
  2,000 generated inputs each, not proven equal.
- **Wrappers shape the result.** The `float()` wrapper is our choice. Without
  it, type-only differences dominate the counts.
- **One platform.** Run on linux-arm64. On x86-64, rapidfuzz loads
  AVX2 or SSE2 builds of the C++ code, which were not tested here.
- **Hand checks are labelled.** The `new york` example, the other-whitespace
  check and the cause are ours, not `equiv` output.
- **Disagreement is not a bug.** None of these is confirmed upstream.
