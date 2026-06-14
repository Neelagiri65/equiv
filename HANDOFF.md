# HANDOFF — equiv

**Last updated:** 2026-06-14
**State:** clean working tree, on `main`. Recent: `8ab5825` (auto-detect changed functions — closes manifest-bypass, issue #1), `97e84a2` (dict/JSON-structural admissible type), `e549dcc` (float gate → AST allowlist).
**Remote:** github.com/Neelagiri65/equiv (local dir is `devtools/equiv-build`). Rust workspace; ships as a GitHub Action (`action.yml`). MIT.

## What this is
A Rust tool — **"an LLM should not be the only thing reviewing LLM-written code."** `equiv` runs a changed function against its previous version on the same deterministically-generated inputs and reports whether behaviour changed. Differential/conformance testing for code review.

## Current state
- Rust workspace (`crates/`, `Cargo.toml`), `conformance/` suite, `examples/`, `docs/`, `action.yml` for CI usage.
- Recent work hardened the review gate: changed-function auto-detection, dict as an admissible structural type, float gate moved to a sound AST allowlist.

## NEXT
- No active task captured — baseline HANDOFF.
- Likely next (from commit trajectory): widen the set of admissible input types beyond dict/float; expand the `conformance/` corpus; surface more of the changed-function detection in the Action output. Check the GitHub issues list for the current queue.
