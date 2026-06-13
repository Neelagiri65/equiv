# examples/

Example code that the project's own review gate runs against.

- `math.py` is checked by the `equiv review` action (`.github/workflows/equiv-review.yml`).
  The root `.equiv-review` manifest lists `examples/math.py : total : int`, so each
  PR that changes `total` is checked against its version on the base branch, reported in
  a PR comment and signed (keyless Sigstore when `id-token: write` is granted).

To check more functions, add lines to `.equiv-review`:

```
<file> : <function> : <arg types>      # int | str | list[int], comma-separated
```
