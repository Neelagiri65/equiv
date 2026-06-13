# equiv

A deterministic checker for code changes that are meant to preserve behaviour.
When a function is refactored, optimised, or rewritten by a person or by an AI,
`equiv` reports whether the new version behaves identically to the old one. If
it does not, `equiv` gives the exact input where they differ. The result is a
reproducible, signed receipt. Re-run the check and you get the same answer,
without trusting any model's opinion.

This matters because most code is now written by AI and reviewed by AI. A model
saying "this looks fine" is not verification. A deterministic check is.

## Quickstart: the PR gate

List the functions whose behaviour must be preserved across a PR in a manifest
at the repository root. The format of each line is
`<file> : <function> : <arg types>`, where arg types are `int`, `str`, or
`list[int]`, comma separated:

```
src/math.py : total : int
```

Add the workflow at `.github/workflows/equiv-review.yml`:

```yaml
on: pull_request
permissions: { contents: read, pull-requests: write, id-token: write }
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - uses: Neelagiri65/equiv@v0.1.0
        with: { keyless: "true" }
```

Pin to a released tag (`@v0.1.0`) rather than `@main` so runs are reproducible
and do not change under you.

Each PR receives a comment. Every changed function is tested against its version
on the base branch. A change that preserves behaviour passes. A change that does
not is reported with the input that distinguishes the two versions. That
fails the check. Receipts are signed with Sigstore keyless signing, which stores
no key. They can be verified with `cosign`.

## CLI

```
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Neelagiri65/equiv/releases/latest/download/equiv-cli-installer.sh | sh
```

```
equiv review candidate.py reference.py <function> <arg types>
equiv verify-receipt <signed-receipt-hex>
```

Exit codes: `0` equivalent, `1` diverges with a printed counterexample, `2`
could not check.

## Scope

`equiv` checks behavioural equivalence of a function against a reference, on
deterministically generated inputs. This is bounded random testing, not
exhaustive verification: a pass means no divergence was found on the generated
inputs, so it can still miss an edge case that only shows up for an input that
was not generated. It does not check intent, architecture, or security. It
cannot judge new functionality that has no reference to compare against. A
passing result means behaviour was preserved on the tested inputs. It does not
mean the change is correct. Supported input types in this version are `int`,
`str` and `list[int]`.

## How it works

Input generation and the verdict are computed in Rust from a fixed seed. The
language runtime is used only as an evaluator and never decides anything that
reaches the receipt, so receipts are identical across hosts. Receipts can be
signed with a local ed25519 key or with keyless Sigstore (OIDC). The keyless
path binds the signature to a verifiable CI identity rather than a stored
secret. The tool is a single static binary with no runtime dependencies,
prebuilt for macOS, Linux and Windows.

## Documentation

- `docs/signing-model.md`: receipt signing with ed25519 and keyless Sigstore.
- `docs/RELEASING.md`: building prebuilt binaries with cargo-dist.
- `equiv/`: the Rust workspace (`equiv-core`, `equiv-engine`, `equiv-review`, `equiv-cli`).

License: Apache-2.0.
