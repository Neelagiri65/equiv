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
  internal audit trail ("our pipeline ran this check before merge") and gating.
- Limits, stated plainly:
  - Not third-party trust. The signer is the same party that ran the check.
  - A long-lived secret is leakable. Rotate it, scope it to the repo, never print it.
  - The public key travels inside the signed receipt. A signature that matches
    it shows only that the envelope is self-consistent: anyone can make a key
    and sign any receipt with it. Who signed is established by comparing that
    key with one you already trust. Consumers must pin the expected key
    (`--signer`, below). The comment surfaces the key for exactly this.

#### Checking a signed receipt
```
equiv verify-receipt <signed-receipt-hex> --signer <public-key-hex>
```
- The envelope must carry its version key and the payload must be an equiv
  receipt in a layout this version knows (a review receipt or a WASM check
  receipt). Signed bytes that are not a receipt are rejected as "not an equiv
  receipt" (exit `3`), whoever signed them.
- With `--signer`, a signature by any other key fails (exit `1`). A signature
  that does not match fails (exit `1`).
- Without `--signer` the command still exits `0` for a self-consistent
  receipt, but says in its output that the signature matches the key carried
  inside the receipt, that this does not show who signed it, and how to pin.
  Do not gate on the unpinned form.
- The output shows what the receipt says (kind, checker, verdict, source
  digests), so you see what was signed. A signed receipt can record a
  counterexample or a could-not-check; exit `0` here is about the signature,
  not the verdict.
- The receipt is decoded strictly against this version's layout, so a receipt
  written by a version with a different layout is rejected rather than
  half-read.

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
  so nothing is signed there and the job log carries a warning that says so.
  Forks get no credential on either path.
- Tested offline: the in-toto Statement schema, deterministic emission,
  hostile-input escaping and the DSSE PAE plus signature roundtrip are unit
  tested in `equiv-core` (`tests/attest.rs`). The live Fulcio and Rekor
  round-trip only runs in real CI, which needs network and OIDC. That is by
  design, since it is cosign's job.

## Security invariants the implementation holds
1. Forks get no secret, by design. GitHub withholds secrets from fork PRs to prevent
   exfiltration. So `EQUIV_SIGNING_KEY` is empty there. The tool then degrades to
   unsigned, never an error. Never use `pull_request_target` to force signing on
   forks; that is the canonical CI secret-exfiltration hole.
2. The secret never hits a log. It is read by the binary from the environment,
   never passed on a command line, never echoed by the entrypoint. Actions also
   masks registered secrets.
2a. The key is not in the environment of the code under review. equiv runs the
   candidate and reference functions in a Python process whose environment is an
   allowlist (`PATH`, `HOME`, temp dirs, locale, `PYTHON*`, virtualenv/conda and
   the Windows system variables; see `EVALUATOR_ENV_ALLOW` in
   `crates/equiv-review/src/lib.rs`). `EQUIV_SIGNING_KEY`, `GH_TOKEN`, the
   Actions OIDC request token and anything else are dropped. Before v0.16.0
   this did not hold: the evaluator inherited the full environment, so a
   candidate could read the key and even return it, which would print it in
   the counterexample line of the PR comment.
   What this is not: a sandbox. The evaluated code runs as the same user as the
   job, with the runner's filesystem and network. Code that sets out to find a
   credential can still look where that user can look: the token
   `actions/checkout` leaves in `.git/config` unless `persist-credentials:
   false` is set (set it), files in the workspace, and on Linux the environment
   of other processes of the same user. The allowlist protects against code
   that never meant to touch the key (AI-written functions, compromised
   imports), not against a hostile PR. Someone with write access to the repo
   could already change the workflow itself; fork PRs get no secrets at all.
2b. The base version of each file is written to a directory created for that
   run only, under a name that cannot be guessed, readable by the job's user
   only, and removed when the review ends. It used to be a guessable path in
   the shared temp directory that the code under review could overwrite. The
   same limit applies as in 2a: this is not a sandbox, and code that runs as
   the same user can still search the temp directory.
2c. The Action's binary is checked before it runs. The release archive is
   compared with the `.sha256` file published in the same release, and the job
   stops on a mismatch or a missing checksum. Because the checksum comes from
   the same release as the archive, this catches a corrupted or partly replaced
   download, not a release that was replaced as a whole; pin the Action by
   commit SHA for that. The fallback build uses `cargo build --locked`. The
   third-party actions the Action and this repo's workflows use are pinned by
   commit SHA.
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
signing is correct and sufficient as v0, provided the invariants above hold
and the consumer pins the signer. It is not sufficient for portable trust by third parties.
That requires the keyless OIDC path, which is the planned v1 and which the
current envelope is forward compatible with. The honest framing, provenance not
correctness and self-attestation not third-party trust, must travel with the
feature so it is never oversold.
