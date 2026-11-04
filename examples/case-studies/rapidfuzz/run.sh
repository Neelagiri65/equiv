#!/bin/bash
# Re-run the rapidfuzz case study. Needs docker and network for the downloads;
# the checks themselves run with --network none.
set -euo pipefail
cd "$(dirname "$0")"
EQUIV_VERSION=v0.15.0
IMAGE=python:3.12@sha256:17b9724493d916f56565c131f389faff2597f46dec6ce09516f95125bc9f519f
case "$(uname -m)" in
  arm64|aarch64) T=aarch64-unknown-linux-gnu; P=manylinux_2_28_aarch64 ;;
  *)             T=x86_64-unknown-linux-gnu;  P=manylinux_2_28_x86_64 ;;
esac
W="$PWD/.work"; mkdir -p "$W"   # reused across runs; docker must be able to mount it
gh release download "$EQUIV_VERSION" -R Neelagiri65/equiv -p "equiv-cli-$T.tar.xz" -D "$W" --clobber
( cd "$W" && gh release download "$EQUIV_VERSION" -R Neelagiri65/equiv -p "equiv-cli-$T.tar.xz.sha256" --clobber && shasum -a 256 -c "equiv-cli-$T.tar.xz.sha256" 2>/dev/null )
tar xf "$W/equiv-cli-$T.tar.xz" -C "$W"
for v in 3.14.6; do
  python3 -m pip download "rapidfuzz==$v" --only-binary :all: --no-deps \
    --python-version 3.12 --platform "$P" -d "$W/wheels-$v" -q
  docker run --rm --network none -v "$PWD":/w -v "$W/equiv-cli-$T":/equiv:ro \
    -v "$W/wheels-$v":/wheels:ro "$IMAGE" bash /w/inside.sh "$v" > "rerun-$v.txt" 2>&1
  echo "rapidfuzz $v: rerun-$v.txt"
done
echo "Compare with: diff <(grep receipt-id out-3.14.6.txt) <(grep receipt-id rerun-3.14.6.txt)"
