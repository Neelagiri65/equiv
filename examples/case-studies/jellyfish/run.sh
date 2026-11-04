#!/bin/bash
# Re-run the jellyfish case study. Needs docker and network for the downloads;
# the checks themselves run with --network none.
set -euo pipefail
cd "$(dirname "$0")"
EQUIV_VERSION=v0.14.0
IMAGE=python:3.12@sha256:17b9724493d916f56565c131f389faff2597f46dec6ce09516f95125bc9f519f
case "$(uname -m)" in
  arm64|aarch64) T=aarch64-unknown-linux-gnu; P=manylinux2014_aarch64 ;;
  *)             T=x86_64-unknown-linux-gnu;  P=manylinux2014_x86_64 ;;
esac
W=$(mktemp -d)
gh release download "$EQUIV_VERSION" -R Neelagiri65/equiv -p "equiv-cli-$T.tar.xz" -D "$W"
( cd "$W" && gh release download "$EQUIV_VERSION" -R Neelagiri65/equiv -p "equiv-cli-$T.tar.xz.sha256" && shasum -a 256 -c "equiv-cli-$T.tar.xz.sha256" 2>/dev/null )
tar xf "$W/equiv-cli-$T.tar.xz" -C "$W"
for v in 1.2.1 1.3.0; do
  python3 -m pip download "jellyfish==$v" --only-binary :all: --no-deps \
    --python-version 3.12 --platform "$P" -d "$W/wheels-$v" -q
  docker run --rm --network none -v "$PWD":/w -v "$W/equiv-cli-$T":/equiv:ro \
    -v "$W/wheels-$v":/wheels:ro "$IMAGE" bash /w/inside.sh "$v" > "rerun-$v.txt" 2>&1
  echo "jellyfish $v: rerun-$v.txt"
done
echo "Compare with: diff <(grep receipt-id out-1.2.1.txt) <(grep receipt-id rerun-1.2.1.txt)"
