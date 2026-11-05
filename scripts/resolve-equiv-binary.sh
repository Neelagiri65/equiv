#!/usr/bin/env bash
# Resolve the equiv binary for the Action, matching the version the user pinned.
#
# The Action's own checkout (ACTION_PATH) is exactly the code the workflow pinned
# (`uses: Neelagiri65/equiv@vX.Y.Z`). Its Cargo.toml names that version, and the
# release vX.Y.Z is built from the same tag, so we download exactly that release.
# If it is not available (unreleased code on a branch, a fork, no access), we build
# the pinned source instead. We never fall back to "latest": a different engine
# would change results under a pinned workflow.
#
# Inputs (env): ACTION_PATH (required), EQUIV_ACTION_REPO (default
# Neelagiri65/equiv), EQUIV_TARGET (default x86_64-unknown-linux-gnu), GITHUB_ENV
# (required; EQUIV_BIN is appended to it), GH_TOKEN (used by gh).
set -euo pipefail

ACTION_PATH="${ACTION_PATH:?ACTION_PATH is required}"
GITHUB_ENV="${GITHUB_ENV:?GITHUB_ENV is required}"
REPO="${EQUIV_ACTION_REPO:-Neelagiri65/equiv}"
TARGET="${EQUIV_TARGET:-x86_64-unknown-linux-gnu}"

VERSION="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$ACTION_PATH/Cargo.toml" | head -1)"
if [[ -z "$VERSION" ]]; then
  echo "equiv: cannot read the engine version from $ACTION_PATH/Cargo.toml; refusing to guess." >&2
  exit 1
fi
TAG="v$VERSION"
ASSET="equiv-cli-${TARGET}.tar.xz"
TMP="$(mktemp -d)"

if gh release download "$TAG" --repo "$REPO" --pattern "$ASSET" --dir "$TMP" 2>/dev/null; then
  tar -xJf "$TMP/$ASSET" -C "$TMP"
  BIN="$(find "$TMP" -type f -name equiv | head -1)"
  chmod +x "$BIN"
  echo "equiv: using the prebuilt $TAG binary from $REPO (matches the pinned Action)."
else
  echo "equiv: no prebuilt $TAG release available; building the pinned source (slower)."
  ( cd "$ACTION_PATH" && cargo build --release --bin equiv )
  BIN="$ACTION_PATH/target/release/equiv"
fi

echo "EQUIV_BIN=$BIN" >> "$GITHUB_ENV"
