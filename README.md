# equiv

**Did this rewrite change what the function does? equiv runs both versions and shows you.**

You rewrite a function, or an AI does it for you. `equiv` runs the old version and
the new version on the same few thousand inputs. If they ever give different
answers, it prints the exact input where they differ. If they never do, it says
so, along with how many inputs it tried.

Every result comes with a receipt. Anyone can re-run the receipt on any machine
and get the same answer, byte for byte. There is no model's opinion involved.

![equiv catches an AI refactor diverging at n = -5, then emits a signed receipt that re-runs to an identical id](assets/equiv-aha.gif)

## What it is good at, and what it skips

`equiv` checks **plain Python functions**: ones that take simple values
(`int`, `str`, `float`, `bool`, `dict`, `list[int]`, `list[str]`) and return a result.

On those, it aims never to raise a false alarm. Across the 36 rewrites in our
test corpus that do keep behaviour, it has raised none.

Anything it cannot judge fairly is listed as **not checked**, never shown as a
pass. That includes:

- methods on classes, async functions and nested functions
- functions whose answer depends on maths-library rounding (`sin`, `exp`, `log`, `pow`)
- return values it cannot compare reliably (sets, custom objects)

A pass means "no difference found on the inputs tried". It does not mean the
code is correct or does what you intended.

## Does it actually catch bugs?

- **Test corpus.** 107 labelled pairs of functions where we already know the
  answer. Of the 71 that really change behaviour, `equiv` catches 63, misses 4,
  and cannot check 4. Of the 36 that keep behaviour, it raises 0 false alarms
  (35 pass, 1 cannot check). Every miss is listed. [`divergences/`](divergences/)
- **[jellyfish](docs/case-studies/jellyfish.md).** This library ships every
  function twice, once in Rust and once in Python. `equiv` finds the two
  disagreeing in 8 of 10 functions. It also catches a bug that shipped in
  version 1.2.1 and was later fixed.
- **[rapidfuzz](docs/case-studies/rapidfuzz.md)** (about 116M downloads a month).
  Its C++ and Python versions score text differently when it contains a
  non-breaking space.
- **Six common refactor mistakes**, run live. Five caught with the exact input,
  one honestly missed. [`examples/scenarios/`](examples/scenarios/)

![equiv across six real-world bug patterns: five caught, one honest miss](assets/equiv-scenarios.gif)

## Use it on pull requests

Add `.github/workflows/equiv-review.yml`:

```yaml
on: pull_request
permissions: { contents: read, pull-requests: write, id-token: write }
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - uses: Neelagiri65/equiv@v0.19.1
        with: { keyless: "true" }
```

That is all. On every PR, `equiv` finds the changed functions, checks each one
against its version on the base branch, and posts one comment. Besides its own
generated inputs, it tries the literal values your tests and other code already
call each function with (read, never run):

- **behaviour preserved**: the check passes
- **behaviour changed**: the check fails, and the comment shows the input that tells the two apart
- **not checked**: listed with the reason (for example, a parameter needs a type hint)

To check only specific functions, list them in `.equiv-review` at the repo root,
one per line: `src/math.py : total : int`.

Pin a released tag (`@v0.19.1`), not `@main`, so results do not change under you.

## Use it from the command line

```
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Neelagiri65/equiv/releases/latest/download/equiv-cli-installer.sh | sh

equiv review new.py old.py <function> <arg types>
equiv verify-receipt <signed-receipt-hex>
```

Exit codes: `0` same behaviour, `1` different (the input is printed), `2` could
not check.

## Good to know

- **Credentials are withheld from the code being checked.** Since v0.16.0 it
  runs with a short allowlist of environment variables, so the signing key,
  `GH_TOKEN` and the Actions login token are not passed to it. It still runs
  with normal file and network access, so only review code you would be
  willing to run. [Details](docs/signing-model.md)
- **No tracking.** `equiv` sends nothing anywhere. Optional local counts can be
  switched on with `EQUIV_METRICS=on`. [Details](docs/metrics.md)
- **Same answer everywhere.** Inputs and verdicts are worked out in Rust from a
  fixed seed. Python only runs the functions. Receipts can be signed with your
  own key or with Sigstore, which needs no stored key.

## More detail

- [How inputs are chosen, limits and budgets](docs/scope.md)
- [How receipts are signed](docs/signing-model.md)
- [Releasing](docs/RELEASING.md)

License: Apache-2.0.
