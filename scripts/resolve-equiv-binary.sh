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

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# The archive and its published checksum (cargo-dist uploads `<asset>.sha256`
# next to every archive) are fetched in one call. The checksum comes from the
# same release as the archive: it catches a corrupted or partly replaced
# download, not a release that was replaced as a whole. Pin the Action by commit
# SHA if that matters to you.
if gh release download "$TAG" --repo "$REPO" --pattern "$ASSET" --pattern "$ASSET.sha256" --dir "$TMP" 2>/dev/null \
   || [[ -f "$TMP/$ASSET" ]]; then
  # An archive without a checksum, or one that does not match, is never run and
  # never replaced by a build: something is wrong with the release.
  if [[ ! -f "$TMP/$ASSET" || ! -s "$TMP/$ASSET.sha256" ]]; then
    echo "::error::equiv: release $TAG of $REPO has no usable $ASSET with its .sha256; refusing to run an unchecked binary." >&2
    exit 1
  fi
  WANT="$(awk '{print tolower($1); exit}' "$TMP/$ASSET.sha256")"
  GOT="$(sha256_of "$TMP/$ASSET")"
  if [[ ! "$WANT" =~ ^[0-9a-f]{64}$ || "$WANT" != "$GOT" ]]; then
    echo "::error::equiv: checksum mismatch for $ASSET from $TAG (published: ${WANT:-none}, downloaded: $GOT); refusing to run it." >&2
    exit 1
  fi
  tar -xJf "$TMP/$ASSET" -C "$TMP"
  BIN="$(find "$TMP" -type f -name equiv | head -1)"
  if [[ -z "$BIN" ]]; then
    echo "::error::equiv: $ASSET from $TAG holds no equiv binary." >&2
    exit 1
  fi
  chmod +x "$BIN"
  echo "equiv: using the prebuilt $TAG binary from $REPO (matches the pinned Action; sha256 $GOT matches the release's checksum file)."
else
  echo "equiv: no prebuilt $TAG release available; building the pinned source with its Cargo.lock (slower)."
  ( cd "$ACTION_PATH" && cargo build --release --locked --bin equiv )
  BIN="$ACTION_PATH/target/release/equiv"
fi

echo "EQUIV_BIN=$BIN" >> "$GITHUB_ENV"
