# equiv across real-world bug patterns

Six refactors that looked behaviour-preserving. Each passed review and passed the tests.
Each one is run through `equiv` live. Five it catches. One it honestly cannot.
Every result here is reproducible with the files in this folder.

![equiv across real-world scenarios](media/equiv-scenarios.gif)

## Scoreboard

| # | Scenario | Domain | equiv verdict | Honest note |
|---|----------|--------|---------------|-------------|
| 1 | Stripe zero-decimal currency (`amount * 100`) | payments | CAUGHT | Exact values. Real: Drupal commerce_stripe #2913605. |
| 2 | Tax personal-allowance floor dropped | tax/payroll | CAUGHT | Phantom negative tax below the allowance. |
| 3 | Gauss sum, negative clamp dropped | arithmetic | CAUGHT | Tests at 1, 5, 10 pass. Only negatives break. |
| 4 | Antimeridian guard added | geospatial | CAUGHT | Real: chrisveness/geodesy v1.1.2 #44. New code throws past +/-180. |
| 5 | Binary-search midpoint overflow | integer overflow | MISSED | Out of scope. Python ints are unbounded. |
| 6 | Average, empty-list guard dropped | edge guard | CAUGHT | New code divides by zero on `[]`; old returns 0. |

## What this shows, honestly

**equiv is value-accurate.** For scenarios 1 to 4 and 6, equiv returns the exact
diverging input and the exact old and new outputs, matching real Python. That includes
the exception cases (scenarios 4 and 6): it shows the side that raised and the real
value the other side returned (`-179`, `0`).

**equiv says EQUIVALENT when it genuinely cannot see a bug.** Scenario 5 is the famous
`(low + high) / 2` overflow. That bug exists in C and Java, where ints are fixed-width.
Python ints are unbounded. The two functions really are equivalent in Python. equiv says so. It does not invent a bug that the language cannot have. A real scope limit,
stated plainly.

**One scope limit (reproducible here):**

**No fixed-width overflow.** See scenario 5. If your bug only exists under 32 or 64-bit
integer wraparound, running the Python form will not surface it. Python ints do not wrap.

(An earlier version of this showcase noted a second issue: an exception on one side
printed the other side as `None`. That display bug is fixed in #7; the exception cases
now show the real value.)

This honesty is the point. equiv tells you what it checked and nothing more.

## Reproduce any scenario

Build the CLI once (`cargo build --release -p equiv-cli`), then:

```
equiv review stripe-jpy/candidate.py            stripe-jpy/reference.py            to_minor_units int
equiv review tax-allowance-floor/candidate.py   tax-allowance-floor/reference.py  tax            int
equiv review gauss-clamp/candidate.py           gauss-clamp/reference.py          total          int
equiv review geodesy-antimeridian/candidate.py  geodesy-antimeridian/reference.py normalize_lon  int
equiv review binary-search-overflow/candidate.py binary-search-overflow/reference.py mid          int,int
equiv review empty-collection-guard/candidate.py empty-collection-guard/reference.py average      list[int]
```

Add `--sign` (with `EQUIV_SIGNING_KEY` set) for a signed receipt you can verify with
`equiv verify-receipt`.

## Sources for the real incidents

- Stripe zero-decimal currencies: https://docs.stripe.com/currencies and https://www.drupal.org/project/commerce_stripe/issues/2913605
- geodesy antimeridian: https://github.com/chrisveness/geodesy/issues/44
- binary-search overflow: https://research.google/blog/extra-extra-read-all-about-it-nearly-all-binary-searches-and-mergesorts-are-broken/
