#!/usr/bin/env bash
# Run the equiv review oracle over a PR's changed functions and post the result
# as a single sticky comment (created once, edited on every push).
set -euo pipefail

BIN="${EQUIV_BIN:-equiv}"
MANIFEST="${EQUIV_MANIFEST:-.equiv-review}"
AUTO="${EQUIV_AUTO:-false}"
BASE="${EQUIV_BASE_INPUT:-}"
FAIL="${EQUIV_FAIL:-true}"
FAIL_NOT_CHECKED="${EQUIV_FAIL_NOT_CHECKED:-false}"

# In manifest mode, a missing manifest means there is nothing to review. In auto
# mode there is no manifest: changed functions come from the diff.
if [[ "$AUTO" != "true" && ! -f "$MANIFEST" ]]; then
  echo "equiv-review: no manifest at '$MANIFEST' and auto is off; skipping."
  exit 0
fi

# Resolve the base ref: explicit input wins, else the PR base, else origin/HEAD.
if [[ -z "$BASE" ]]; then
  BASE="origin/${GITHUB_BASE_REF:-main}"
fi
# Ensure the base revision is present (Actions checkouts are often shallow).
git fetch --no-tags --depth=1 origin "${GITHUB_BASE_REF:-main}" 2>/dev/null || true

# Run the oracle. It prints the Markdown comment; its exit code is the gate.
# EQUIV_SIGNING_KEY (if set) is read by the binary from the environment and is
# never passed on the command line or echoed here. When unset (e.g. fork PRs,
# where GitHub withholds secrets), receipts are left unsigned. This is not an error.
set +e
if [[ "$AUTO" == "true" ]]; then
  COMMENT="$("$BIN" review-pr --auto --base "$BASE" \
    --receipts-out equiv-receipts.tsv \
    --attest-out equiv-attestations)"
else
  COMMENT="$("$BIN" review-pr "$MANIFEST" --base "$BASE" \
    --receipts-out equiv-receipts.tsv \
    --attest-out equiv-attestations)"
fi
CODE=$?
set -e

echo "$COMMENT"

# Keyless signing (recommended). cosign uses the Actions OIDC identity to get a
# short-lived Fulcio cert and logs to Rekor, with no long-lived secret. Each
# in-toto statement is signed into a .sigstore bundle next to it.
if [[ "${EQUIV_KEYLESS:-false}" == "true" ]] && command -v cosign >/dev/null 2>&1; then
  shopt -s nullglob
  for stmt in equiv-attestations/*.intoto.json; do
    COSIGN_EXPERIMENTAL=1 cosign sign-blob --yes \
      --bundle "${stmt%.json}.sigstore" "$stmt" >/dev/null 2>&1 \
      && echo "keyless-signed: $stmt" \
      || echo "keyless sign failed (need id-token: write?) for $stmt"
  done
  shopt -u nullglob
fi

# Post / update the sticky comment (PR context only).
PR_NUMBER="$(jq -r '.pull_request.number // empty' "${GITHUB_EVENT_PATH:-/dev/null}" 2>/dev/null || true)"
if [[ -n "${PR_NUMBER:-}" && -n "${GH_TOKEN:-}" ]]; then
  MARKER="<!-- equiv-review -->"
  EXISTING="$(gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --jq ".[] | select(.body | contains(\"$MARKER\")) | .id" 2>/dev/null | head -n1 || true)"
  if [[ -n "$EXISTING" ]]; then
    gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${EXISTING}" \
      -f body="$COMMENT" >/dev/null
    echo "equiv-review: updated comment $EXISTING"
  else
    gh api -X POST "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
      -f body="$COMMENT" >/dev/null
    echo "equiv-review: posted new comment"
  fi
fi

# Gate policy (backlog 0.3c). The binary's exit code is a plain report:
#   0  every changed function was checked and preserved behaviour
#   1  at least one function changed behaviour (a counterexample is in the comment)
#   2  nothing diverged, but at least one function was NOT checked (safety stop,
#      refused type, error, or a changed function auto-detect could not check)
#   3+ the review itself did not run (bad manifest, no git repo, bad key)
# Only a divergence blocks the merge by default. "Not checked" is listed in
# the comment, never shown as green, and blocks only with fail-on-not-checked.
# A run that did not happen always fails: a misconfigured check must not pass.
case "$CODE" in
  0) exit 0 ;;
  1)
    if [[ "$FAIL" == "true" ]]; then
      echo "::error::equiv-review: a function changed behaviour; see the PR comment (exit 1)"
      exit 1
    fi
    echo "::notice::equiv-review: a function changed behaviour but fail-on-divergence is off (exit 1 not enforced)"
    exit 0 ;;
  2)
    if [[ "$FAIL_NOT_CHECKED" == "true" ]]; then
      echo "::error::equiv-review: some changed functions were not checked and fail-on-not-checked is on (exit 2)"
      exit 2
    fi
    echo "::notice::equiv-review: nothing diverged, but some changed functions were not checked (listed in the PR comment). Not blocking the merge; set fail-on-not-checked: \"true\" to block."
    exit 0 ;;
  *)
    echo "::error::equiv-review: the review did not run (exit $CODE); this is never a pass"
    exit "$CODE" ;;
esac
