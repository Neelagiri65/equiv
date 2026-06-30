# HANDOFF — equiv

## 2026-06-30: exception-display fix, real-world showcase, launch, repo hardening

- **Fix (PR #7):** the review driver printed the exception-name columns for both
  sides of a counterexample. The non-raising side then reported `None` instead of
  its real value (e.g. `reference -> None` where the value was `0`). Now each side
  shows its exception name if it raised, else its actual value. Verdict logic
  unchanged. Regression test `exception_on_one_side_reports_other_sides_real_value`.
- **Real-world scenario showcase (PR #6):** `examples/scenarios/` runs six
  documented bug patterns through equiv live (Stripe JPY x100, tax allowance floor,
  Gauss clamp, geodesy antimeridian, binary-search overflow [honest miss],
  empty-list guard) with runnable files, a branded GIF + carousel and an honest
  README. Result: five caught, one honest miss.
- **README GIF (PR #8):** "Does it actually catch the bug?" branded animation.
- **Marketplace (PR #9):** `branding.color` purple -> blue (Nativerse). The action
  is Marketplace-eligible; publishing is the UI release step.
- **Repo hardening:** `main` branch protection applied. Force-push and deletion
  blocked, the three `tests` matrix checks required, `enforce_admins: false` so the
  owner is never locked out, conversation resolution required.
- **Launch:** LinkedIn post live, Nativerse-branded yen-story card (the Stripe
  zero-decimal 100x overcharge, framed as the AI-refactor-that-passed-review).
  Social assets in `devtools/equiv/social/`.
- **Limitations found and logged to the vault:** (1) domain-blindness + exact
  equality (generates the whole declared type incl. out-of-domain inputs; treats
  int `0` and float `0.0` as different); (2) no fixed-width integer overflow (Python
  ints are unbounded); (3) the exception-display bug, now fixed. Vault:
  `learnings/discoveries/equiv-evaluator-fidelity-map-2026-06-30`,
  `equiv-domain-blindness-and-equality-semantics`,
  `raw/research/equiv-real-world-mishap-incidents-2026-06-30`.

**Shipped (all complete):**
- **v0.2.1 released.** Tagged `v0.2.1`; cargo-dist built and published binaries for
  macOS arm64/x64, Linux x64/arm64, Windows x64, with installers and checksums. A
  version bump drifts the two pinned golden receipt-ids (`checker_version` is in every
  receipt); both were updated in #11. Keep this in mind for the next bump.
- **Published to the GitHub Marketplace** from the v0.2.1 release (blue branding, the fix).
- README quickstart repinned `@v0.1.0` -> `@v0.2.1` (#12).
- `main` branch protection on: force-push and deletion blocked, `test` matrix checks
  required, `enforce_admins: false` so the owner is not locked out.
- Launched on LinkedIn (Nativerse-branded Stripe-100x story). Real-incident research
  and four equiv limitation write-ups saved to the agent-vault.

**Next steps:**
- Follow-up "proof" LinkedIn post: the carousel/GIF (five caught, one honest miss).
  Assets ready in `~/devtools/equiv/social/shots/`; caption not yet drafted.
- Watch inbound: LinkedIn comments, repo issues, stars, Marketplace installs.
- Strategic fork: equiv is an equivalence oracle, the market keeps asking for
  correctness. Decide between staying narrow-and-honest and adding a
  properties/metamorphic mode (costs setup, never market as zero-setup).

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
