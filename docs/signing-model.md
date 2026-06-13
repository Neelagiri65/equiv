# equiv Receipt Signing Model

## What a signature attests and what it omits
A receipt is deterministic and re-runnable. Anyone can re-run `equiv review-pr`
and get the identical `receipt-id` (the SHA-256 of the canonical receipt bytes).
So the signature does not establish integrity. Re-execution already does that.

The signature adds provenance: the holder of this key produced exactly these
bytes. It is for a consumer who cannot or will not re-run the check, such as an
auditor, a compliance log, or a downstream gate. It attests who vouched, not that
the verdict is correct. This matches the project's spine: claim less, stay
checkable.

## The two signing models

### v0 (shipped): long-lived ed25519 key in a CI secret
- `EQUIV_SIGNING_KEY` (a 64 hex seed) is a repo or org Actions secret.
  `equiv review-pr` signs each receipt and writes them to `equiv-receipts.tsv`,
  uploaded as a CI artifact. ed25519 is deterministic (RFC 8032), so signing
  preserves the byte-identical-receipt property.
- This is self-attestation. The org signs its own receipts. Honest use: an
  internal audit trail ("our pipeline verified this before merge") and gating.
- Limits, stated plainly:
  - Not third-party trust. The signer is the same party that ran the check.
  - A long-lived secret is leakable. Rotate it, scope it to the repo, never print it.
  - The public key in the receipt identifies the signer. Consumers should pin
    the expected key. The comment surfaces it for exactly this.

### v1 (built, recommended): keyless Sigstore / OIDC
- The CI run authenticates with its workload identity (GitHub issues every
  Action an OIDC token). `cosign` exchanges it at Fulcio for a short-lived
  signing certificate, signs, then logs to the Rekor transparency log. No
  long-lived secret to leak.
- The signer becomes a verifiable identity, for example
  `https://github.com/OWNER/REPO/.github/workflows/equiv-review.yml@refs/heads/main`.
  It is not an anonymous key. That is what makes the receipt portable for trust
  by third parties.
- Architecture: equiv owns only the payload, a standard in-toto v1 Statement
  wrapping the review verdict as a predicate (`equiv-core::attest`, predicateType
  `https://equiv.dev/attestations/review/v1`, subject digest is the reviewed
  source's sha256). The keyless crypto is delegated to `cosign`, the standard
  Sigstore client. equiv does not reimplement Fulcio or Rekor.
  `equiv review-pr --attest-out <dir>` emits one `<fn>.intoto.json`
  per function; the Action's `keyless: true` runs
  `cosign sign-blob --yes --bundle <fn>.sigstore <fn>.intoto.json`.
- Verify (anyone, no equiv install):
  ```
  cosign verify-blob \
    --bundle clamp.intoto.sigstore \
    --certificate-identity-regexp '^https://github.com/OWNER/REPO/' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    clamp.intoto.json
  ```
- Requires `permissions: id-token: write` in the calling workflow. Does it work
  on fork PRs? No. The `id-token` is withheld from forks, the same as secrets,
  so the step no-ops there. This is the same fork-safe property as v0.
- Tested offline: the in-toto Statement schema, deterministic emission,
  hostile-input escaping and the DSSE PAE plus signature roundtrip are unit
  tested in `equiv-core` (`tests/attest.rs`). The live Fulcio and Rekor
  round-trip only runs in real CI, which needs network and OIDC. That is by
  design, since it is cosign's job.

## Security invariants the implementation holds
1. Fork-safe by design. GitHub withholds secrets from fork PRs to prevent
   exfiltration. So `EQUIV_SIGNING_KEY` is empty there. The tool then degrades to
   unsigned, never an error. Never use `pull_request_target` to force signing on
   forks; that is the canonical CI secret-exfiltration hole.
2. The secret never hits a log. It is read by the binary from the environment,
   never passed on a command line, never echoed by the entrypoint. Actions also
   masks registered secrets.
3. Replay is neutralised by content addressing. The receipt binds
   `candidate_sha256 + reference_sha256 + spec`, the actual source hashes. A
   replayed signature only validates on identical sources, where the verdict
   genuinely holds. No PR-number binding is needed.
4. The durable artifact, not the comment, is the record. The PR comment is
   mutable and human facing; it shows the signer and receipt ids. The signed
   envelopes live in `equiv-receipts.tsv` (a CI artifact) for audit or registry
   use.

## Verdict on "is the approach right?"
For the stated job, gating a PR or an internal audit trail, CI-secret ed25519
signing is correct and sufficient as v0, provided the four invariants above
hold, which they do. It is not sufficient for portable trust by third parties.
That requires the keyless OIDC path, which is the planned v1 and which the
current envelope is forward compatible with. The honest framing, provenance not
correctness and self-attestation not third-party trust, must travel with the
feature so it is never oversold.
