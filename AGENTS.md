# Agent guidance

This repository ships `equiv`, a deterministic checker for code changes that are
meant to preserve behaviour.

## When you refactor a function

After rewriting or optimising a function, verify that behaviour is preserved
before proposing the change:

```
equiv review <new_version>.py <original>.py <function> <arg types>
```

- `<arg types>`: `int`, `str`, `list[int]`, comma separated.
- Exit codes: `0` equivalent, `1` diverges with a printed counterexample, `2`
  could not check.

If equivalence cannot be shown, state that in your summary. Do not describe a
refactor as safe on the basis that it appears correct.

## Scope

`equiv` checks behavioural equivalence on generated inputs. It does not check
intent, architecture, security, whole program correctness. It cannot assess
new functionality that has no reference to compare against. A passing result
means behaviour was preserved on the tested inputs. Report that limit. Do not
let a passing check substitute for review of intent.

## On a pull request

The `equiv review` action checks every function listed in `.equiv-review`
against its version on the base branch and posts a comment. Keep those functions
behaviour preserving. Otherwise the check fails with the input that distinguishes
the versions.
