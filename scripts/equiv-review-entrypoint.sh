#!/usr/bin/env bash
# Run the equiv review oracle over a PR's changed functions and post the result
# as a single sticky comment (created once, edited on every push).
#
# This wrapper decides nothing about behaviour. The binary's exit code is the
# report; the wrapper maps it to a job result under the inputs, and treats
# everything it cannot understand (an input that is not true/false, a base ref
# that cannot be read, a review that did not run) as a failure, never a pass.
set -euo pipefail

BIN="${EQUIV_BIN:-equiv}"
MANIFEST="${EQUIV_MANIFEST:-.equiv-review}"
BASE="${EQUIV_BASE_INPUT:-}"
ONLY_LABEL="${EQUIV_ONLY_LABEL:-}"

# A setting that is on must be on however it was typed. "True" used to fall
# through every `== "true"` test and switch the gate off without a word.
# true/false in any letter case are accepted; anything else stops the job.
bool() { # bool <input name> <value>  -> prints true, false or nothing (unset)
  local v
  v="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  case "$v" in
    true|false|"") printf '%s' "$v" ;;
    *)
      echo "::error::equiv-review: input '$1' must be \"true\" or \"false\", got \"$2\". Refusing to guess; the review did not run." >&2
      return 3 ;;
  esac
}
AUTO="$(bool auto "${EQUIV_AUTO:-}")" || exit 3
FAIL="$(bool fail-on-divergence "${EQUIV_FAIL:-}")" || exit 3
FAIL_NOT_CHECKED="$(bool fail-on-not-checked "${EQUIV_FAIL_NOT_CHECKED:-}")" || exit 3
KEYLESS="$(bool keyless "${EQUIV_KEYLESS:-}")" || exit 3
FAIL_NOT_CHECKED="${FAIL_NOT_CHECKED:-false}"
KEYLESS="${KEYLESS:-false}"

# Intent. equiv answers "did behaviour change?", which is only a defect when the
# PR is meant to keep behaviour. With `only-label` set, the review runs only on
# PRs carrying that label (the author declares "no behaviour change"); other PRs
# are skipped. Unlabelled feature PRs change behaviour on purpose, and blocking
# them would only restate the author's intent.
LABELLED="false"
if [[ -n "$ONLY_LABEL" ]]; then
  if [[ -z "${GITHUB_EVENT_PATH:-}" || ! -f "${GITHUB_EVENT_PATH:-}" ]]; then
    echo "::notice::equiv-review: no pull request event, so no '$ONLY_LABEL' label; skipping (only-label is set)."
    exit 0
  fi
  set +e
  python3 - "$GITHUB_EVENT_PATH" "$ONLY_LABEL" <<'PY'
import json, sys
try:
    ev = json.load(open(sys.argv[1]))
    labels = [l.get("name") for l in (ev.get("pull_request") or {}).get("labels", [])]
except Exception as e:
    print(f"cannot read the event file: {e}", file=sys.stderr)
    sys.exit(2)
sys.exit(0 if sys.argv[2] in labels else 1)
PY
  RC=$?
  set -e
  case "$RC" in
    0) LABELLED="true" ;;
    1)
      echo "::notice::equiv-review: PR does not carry the '$ONLY_LABEL' label; skipping (only-label is set)."
      exit 0 ;;
    *)
      # Not knowing whether the label is there is not the same as its absence.
      echo "::error::equiv-review: could not read the PR's labels (exit $RC); the review did not run"
      exit 3 ;;
  esac
fi

# Resolve the base ref: explicit input wins, else the PR base.
if [[ -z "$BASE" ]]; then
  BASE="origin/${GITHUB_BASE_REF:-main}"
fi
case "$BASE" in
  -*) echo "::error::equiv-review: base '$BASE' is not a ref name; the review did not run"; exit 3 ;;
esac
# Make sure the base revision is present (Actions checkouts are often shallow).
# A failed fetch is shown, not hidden; whether the base is usable is decided by
# the binary, which exits 4 when it cannot read it.
if ! git rev-parse --verify --quiet "${BASE}^{commit}" >/dev/null 2>&1; then
  if ! FETCH_OUT="$(git fetch --no-tags --depth=1 origin "${GITHUB_BASE_REF:-main}" 2>&1)"; then
    echo "::warning::equiv-review: could not fetch the base branch: $(printf '%s' "$FETCH_OUT" | tail -n 1)"
  fi
fi

# Mode. `auto` unset (the default): use the manifest if there is one, otherwise
# detect every changed function from the diff, so a repo with no manifest is
# reviewed instead of skipped. "There is one" means at the base ref or in the
# PR: a PR that deletes the manifest is still a manifest review (the binary
# runs the lines listed at base). Explicit "true" ignores the manifest;
# explicit "false" with no manifest anywhere means there is nothing to review.
HAS_MANIFEST="false"
if [[ -f "$MANIFEST" ]] || git cat-file -e "${BASE}:${MANIFEST#./}" 2>/dev/null; then
  HAS_MANIFEST="true"
fi
if [[ -z "$AUTO" ]]; then
  if [[ "$HAS_MANIFEST" == "true" ]]; then AUTO="false"; else AUTO="true"; fi
fi

# Gate default. Unset fail-on-divergence blocks the merge only when the PR has
# declared that behaviour must not change: the functions are listed in the
# manifest, or the PR carries the only-label label. In plain auto-detect mode a
# divergence is reported in the comment but does not block, because most changed
# functions in an ordinary PR change on purpose. Explicit true/false wins.
if [[ -z "$FAIL" ]]; then
  if [[ "$AUTO" != "true" || "$LABELLED" == "true" ]]; then FAIL="true"; else FAIL="false"; fi
fi
if [[ "$AUTO" != "true" && "$HAS_MANIFEST" != "true" ]]; then
  echo "::notice::equiv-review: no manifest at '$MANIFEST' (in this PR or at base) and auto is \"false\"; skipping. Nothing was reviewed."
  exit 0
fi

# Run the oracle. It prints the Markdown comment; its exit code is the report.
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

# No report text: say what happened instead of leaving an earlier comment on
# the PR to speak for this run.
MARKER="<!-- equiv-review -->"
if [[ -z "$COMMENT" ]]; then
  if [[ "$CODE" -eq 0 ]]; then
    COMMENT="$(printf '%s\n## equiv review\n\nNothing to review in this run: no listed or changed function was found.\n' "$MARKER")"
  else
    COMMENT="$(printf '%s\n## equiv review\n\nThe review did not run (exit %s). This is not a result; the reason is in the job log.\n' "$MARKER" "$CODE")"
  fi
fi

# The report always goes to the job log and the job summary, so it does not
# depend on being allowed to comment.
echo "$COMMENT"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  printf '%s\n' "$COMMENT" >> "$GITHUB_STEP_SUMMARY" || true
fi

# Keyless signing (recommended). cosign uses the Actions OIDC identity to get a
# short-lived Fulcio cert and logs to Rekor, with no long-lived secret. Each
# in-toto statement is signed into a .sigstore bundle next to it. Signing is
# not the review: a failure here is a warning and does not change the result.
if [[ "$KEYLESS" == "true" ]]; then
  if command -v cosign >/dev/null 2>&1; then
    shopt -s nullglob
    for stmt in equiv-attestations/*.intoto.json; do
      if COSIGN_EXPERIMENTAL=1 cosign sign-blob --yes \
        --bundle "${stmt%.json}.sigstore" "$stmt" >/dev/null 2>&1; then
        echo "keyless-signed: $stmt"
      else
        echo "::warning::equiv-review: keyless signing failed for $stmt (needs id-token: write; not available on fork PRs). The statement is unsigned."
      fi
    done
    shopt -u nullglob
  else
    echo "::warning::equiv-review: keyless is \"true\" but cosign is not installed; nothing was signed."
  fi
fi

# Post / update the sticky comment (PR context only). On a fork PR the token is
# read-only and this fails; that is a warning. The job result follows the
# review, not the comment.
post_comment() {
  local existing
  existing="$(gh api --paginate "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --jq ".[] | select(.body | contains(\"$MARKER\")) | .id" 2>/dev/null | head -n1 || true)"
  if [[ -n "$existing" ]]; then
    gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${existing}" \
      -f body="$COMMENT" >/dev/null || return 1
    echo "equiv-review: updated comment $existing"
  else
    gh api -X POST "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
      -f body="$COMMENT" >/dev/null || return 1
    echo "equiv-review: posted new comment"
  fi
}
PR_NUMBER="$(jq -r '.pull_request.number // empty' "${GITHUB_EVENT_PATH:-/dev/null}" 2>/dev/null || true)"
if [[ -n "${PR_NUMBER:-}" && -n "${GH_TOKEN:-}" ]]; then
  if ! post_comment; then
    echo "::warning::equiv-review: could not post the PR comment (on a fork PR the token is read-only). The full report is above and in the job summary; the check result is not affected."
  fi
fi

# Gate policy (backlog 0.3c). The binary's exit code is a plain report:
#   0  every listed or changed function was checked and preserved behaviour
#   1  at least one function changed behaviour (a counterexample is in the
#      comment), or a listed function no longer exists
#   2  nothing diverged, but at least one function was NOT checked (safety stop,
#      refused type, error, or a changed function auto-detect could not check)
#   3  the review itself did not run (bad manifest, bad arguments, bad key)
#   4  the base ref could not be read, so nothing was compared
# Only a divergence blocks the merge by default. "Not checked" is listed in
# the comment, never shown as green, and blocks only with fail-on-not-checked.
# A run that did not happen always fails, whatever the inputs say: a
# misconfigured check must not pass.
case "$CODE" in
  0) exit 0 ;;
  1)
    if [[ "$FAIL" == "true" ]]; then
      echo "::error::equiv-review: a function changed behaviour; see the report (exit 1)"
      exit 1
    fi
    echo "::notice::equiv-review: a function changed behaviour; reported, not blocking (auto-detect mode without a manifest or only-label, or fail-on-divergence is \"false\"). If this PR is meant to keep behaviour, list the functions in .equiv-review or set only-label."
    exit 0 ;;
  2)
    if [[ "$FAIL_NOT_CHECKED" == "true" ]]; then
      echo "::error::equiv-review: some changed functions were not checked and fail-on-not-checked is on (exit 2)"
      exit 2
    fi
    echo "::notice::equiv-review: nothing diverged, but some changed functions were not checked (listed in the report). Not blocking the merge; set fail-on-not-checked: \"true\" to block."
    exit 0 ;;
  4)
    echo "::error::equiv-review: the base ref '$BASE' could not be read, so nothing was compared (exit 4). Use fetch-depth: 0 on actions/checkout, or set the 'base' input to a ref that exists. This is never a pass."
    exit 4 ;;
  *)
    echo "::error::equiv-review: the review did not run (exit $CODE); this is never a pass"
    exit "$CODE" ;;
esac
