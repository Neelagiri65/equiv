#!/usr/bin/env bash
# Run the equiv review oracle over a PR's changed functions and post the result
# as a single sticky comment (created once, edited on every push).
set -euo pipefail

BIN="${EQUIV_BIN:-equiv}"
MANIFEST="${EQUIV_MANIFEST:-.equiv-review}"
BASE="${EQUIV_BASE_INPUT:-}"
FAIL="${EQUIV_FAIL:-true}"

if [[ ! -f "$MANIFEST" ]]; then
  echo "equiv-review: no manifest at '$MANIFEST'; skipping."
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
COMMENT="$("$BIN" review-pr "$MANIFEST" --base "$BASE" \
  --receipts-out equiv-receipts.tsv \
  --attest-out equiv-attestations)"
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

if [[ "$CODE" -ne 0 && "$FAIL" == "true" ]]; then
  echo "equiv-review: gate failed (exit $CODE)"
  exit "$CODE"
fi
exit 0
