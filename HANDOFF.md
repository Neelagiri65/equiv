# HANDOFF — equiv

## 2026-06-29: v0.2.0 — smarter input generation (boundary + AST-literal), committed to main
Closed the core weakness: a green pass used to mean only "seeded random darts missed."
`gen_cases` now layers three deterministic stages — all-default base, then per-arg **boundary
corners** + **source-literal branch values** (one-at-a-time), then the original seeded random cases,
deduplicated. New `tests/generation.rs` proves it catches what random missed: a magic constant
(`x == 777`), strings longer than 8, uppercase, lists longer than 6. `Equivalent{n}` now reports the
honest total case count. Commit `d72931a`, **89 tests green, 0 warnings**, flair-gate clean.
- **Determinism held (AC-1):** generation is pure Rust, source scan deterministic, dedup via BTreeSet.
  Both golden receipt-ids updated deliberately (generation change + the 0.1.0→0.2.0 `checker_version`).
- **Documented limits (prosecution):** (1) integer literals capped at the ±1e6 magnitude envelope so a
  linear reference cannot hang — a divergence beyond that is not reached; (2) one-at-a-time arg
  variation misses a divergence that needs two specific args at once. Both in README scope.
- **NOT pushed / NOT released.** Follow-ups: `git push`; cut the **v0.2.0 tag** (RELEASING.md) to build
  prebuilt binaries; then move README quickstart pin `@v0.1.0` → `@v0.2.0`.
- **NEXT build (the natural successor):** deterministic **coverage-guided generation** — hash each
  version's execution path, steer inputs toward control-flow *divergence*; must stay cross-host stable
  (fixed budget, seeded, stable path-hash) to preserve the receipt moat. Related: salvage the shelved
  BMC/symbolic engine as a discriminating-*input* finder (solve for an input hitting a differing branch,
  confirm by running — sound, no trust in the solver). Do NOT swap SHA-256 for BLAKE3 (non-bottleneck;
  ubiquity is the trust asset).

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
