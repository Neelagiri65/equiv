# Scope, inputs and limits

Moved from the README. The README has the short version.

## What is checked

`equiv` checks behavioural equivalence of a function against a reference, on
deterministically generated inputs. Generation has three deterministic layers:
boundary corners per type (empty, sign and off-by-one values, uppercase, strings
longer than 8, lists longer than 6, the float special values), inputs read from
the source's own branch literals (a constant `== 777` in the code is tested
directly, with its neighbours), then seeded random cases. This is still bounded
testing, not exhaustive verification: a pass means no divergence was found on the
generated inputs. Two limits are deliberate. Random integers stay inside a magnitude envelope
(|n| up to 1,000,000) so a linear reference cannot hang; fixed large-magnitude
corners (the 32-bit wrap points, the float64 exactness cliff at 2^53 and the
64-bit limits, each with its negative) are tried after every other input, and
a version that cannot finish one of them inside the step budget has that input
recorded in the receipt as not judged rather than failing the review. Arguments are varied one at a time, which
means a divergence that needs two specific arguments at once may be missed. Methods, async and nested functions are listed as not checked, never judged. A function that did not change is not re-checked when something it calls did. It does not check intent, architecture, security. It
cannot judge new functionality that has no reference to compare against. A
passing result means behaviour was preserved on the tested inputs. It does not
mean the change is correct. Supported input types in this version are `int`,
`str`, `list[int]`, `list[str]`, `float`, `dict`, `bool`. Dict and list results are compared
structurally: a dict is a map (key order ignored); a value that is not
JSON-structural (a set, a custom object, non-string keys) is refused. Float
reviews are admitted only inside the IEEE-754
correctly rounded operations, where the result is identical on every machine. A
function that reaches a transcendental (sin, exp, log, pow) is refused by name
rather than judged. Its last bit is not reproducible across maths libraries.

Each call of either version, and each module load, runs under a step budget:
4,000,000 Python line and jump events by default (enough for a loop with a one-line body over any int in the random range), counted through
`sys.monitoring`. No clock is read, so the same budget gives the same result on
every host running the same Python minor; the receipt records which one ran. A version that exceeds it (an infinite loop, or a loop over a huge
literal-derived input) ends the review as could not check (exit `2`), naming the
budget, the input and the side; a counterexample found on an earlier input still
stands. The budget is written into the receipt. Work done inside C code (for
example `sum(range(10**18))`) raises no Python events and is not counted. For
that case a separate safety limit applies: if one function's check runs longer
than `EQUIV_SAFETY_SECONDS` (default 120) of wall-clock time, its evaluator
processes are stopped and the function is reported as could not check (exit
`2`). This limit is not part of the deterministic budget, so it never decides a
verdict and no receipt (signed or unsigned) is issued for a stopped function;
other functions in the same run still report. The budget needs Python 3.12 or
newer; on an older Python the review reports could not check instead of running
without a limit.

## Exit codes

Exit codes: `0` equivalent, `1` diverges with a printed counterexample, `2`
could not check. For `review-pr` the code is a report over the whole run: `0`
every changed function was checked and preserved behaviour, `1` at least one
changed behaviour, `2` nothing diverged but at least one changed function was
not checked (safety stop, refused type, error, or no supported type hints; each
is listed in the comment, never shown as green). The Action fails the PR check
on `1` by default and on `2` only with `fail-on-not-checked: "true"`; a review
that did not run at all (`3`) always fails it.

## The manifest

The manifest is optional. Without one, every function changed in the PR is
detected from the diff and checked if each parameter has a supported type hint;
the rest are listed as not checked, with the parameters that need a hint. To pin
an exact list instead, write the functions whose behaviour must be preserved
across a PR in a manifest at the repository root. The format of each line is
`<file> : <function> : <arg types>`, where arg types are `int`, `str`,
`list[int]`, `list[str]`, `float`, `dict` or `bool`, comma separated:

```
src/math.py : total : int
```

## How it works

Input generation and the verdict are computed in Rust from a fixed seed. The
language runtime is used only as an evaluator and never decides anything that
reaches the receipt. Receipts are identical across hosts. Receipts can be
signed with a local ed25519 key or with keyless Sigstore (OIDC). The keyless
path binds the signature to a verifiable CI identity rather than a stored
secret. The tool is a single static binary with no runtime dependencies,
prebuilt for macOS, Linux and Windows.
