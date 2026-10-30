#!/usr/bin/env bash
# One command that says whether the repo still holds its founding rules.
# Run locally before pushing, and in CI on every pull request (guard.yml).
#
#   1. tests        cargo test --workspace (includes the cross-host golden receipt)
#   2. corpus       divergence corpus vs divergences/BASELINE.json: no false alarm,
#                   no broken case, no regression
#   3. wording      no "verified" / "safe" / "proved" in strings equiv prints
#   4. identity     every commit in BASE..HEAD is authored and committed by the
#                   maintainer, with no co-author or AI attribution lines
#
# BASE defaults to origin/main. Exit 0 only if all four pass.
set -uo pipefail
cd "$(dirname "$0")/../.."

BASE="${EQUIV_BASE:-origin/main}"
OWNER_NAME="Neelagiri65"
OWNER_EMAIL="neelagiri.chettiyar@nativerse-ventures.com"
fail=0
step() { printf '\n== %s\n' "$1"; }
bad() { printf 'FAIL: %s\n' "$1"; fail=1; }
ok() { printf 'ok: %s\n' "$1"; }

step "1/4 tests"
if cargo test --workspace -q >/tmp/equiv-harness-tests.log 2>&1; then
  ok "cargo test --workspace"
else
  tail -40 /tmp/equiv-harness-tests.log
  bad "cargo test --workspace (log above)"
fi

step "2/4 divergence corpus"
if [ -x scripts/run-divergences.sh ] || [ -f scripts/run-divergences.sh ]; then
  if bash scripts/run-divergences.sh >/tmp/equiv-harness-corpus.log 2>&1; then
    grep -E "^(Catch rate|False-alarm rate|Could not check):" /tmp/equiv-harness-corpus.log
    ok "corpus matches or beats the baseline"
  else
    tail -30 /tmp/equiv-harness-corpus.log
    bad "corpus: false alarm, broken case or regression (log above)"
  fi
else
  ok "no corpus on this branch (skipped)"
fi

step "3/4 wording"
# Quoted string literals in non-test Rust sources and the Action. Comment lines
# are ignored; a line may opt out with the marker  harness:allow-wording.
hits=$(grep -rnE '"[^"]*\b([Vv]erified|[Ss]afe|[Pp]roved)\b[^"]*"' crates/*/src action.yml 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:\s*//' | grep -v 'harness:allow-wording' || true)
if [ -n "$hits" ]; then
  echo "$hits"
  bad "equiv must never describe a result as verified, safe or proved"
else
  ok "no overclaiming words in printed strings"
fi

step "4/4 commit identity ($BASE..HEAD)"
if git rev-parse -q --verify "$BASE" >/dev/null; then
  while IFS=$'\t' read -r sha an ae cn ce; do
    [ -z "$sha" ] && continue
    if [ "$an" != "$OWNER_NAME" ] || [ "$ae" != "$OWNER_EMAIL" ]; then bad "$sha author is $an <$ae>"; fi
    if [ "$cn" != "$OWNER_NAME" ] || [ "$ce" != "$OWNER_EMAIL" ]; then bad "$sha committer is $cn <$ce>"; fi
    if git log -1 --format=%B "$sha" | grep -qiE 'co-authored-by|generated with|claude|anthropic'; then
      bad "$sha message carries co-author or AI attribution"
    fi
  done < <(git log --no-merges --format=$'%h\t%an\t%ae\t%cn\t%ce' "$BASE..HEAD")
  [ "$fail" -eq 0 ] && ok "all commits are the maintainer's, no attribution lines"
else
  ok "base $BASE not found (skipped)"
fi

echo
if [ "$fail" -eq 0 ]; then echo "HARNESS: PASS"; else echo "HARNESS: FAIL"; fi
exit "$fail"
