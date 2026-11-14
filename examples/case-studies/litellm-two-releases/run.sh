#!/bin/bash
# Re-run the LiteLLM two-releases example.
# Needs: equiv 0.32.0 or later on PATH, python3 with the venv module, and network
# for the two pip installs. The reviews themselves send nothing: every call is mocked.
set -euo pipefail
cd "$(dirname "$0")"
W="$PWD/.work"; mkdir -p "$W"
for v in 1.109.1 1.110.0rc2; do
  if [ ! -x "$W/litellm-$v/bin/python" ]; then
    python3 -m venv "$W/litellm-$v"
    "$W/litellm-$v/bin/python" -m pip install -q "litellm==$v"
  fi
done
OLD="$W/litellm-1.109.1/bin/python"
NEW="$W/litellm-1.110.0rc2/bin/python"
# Importing LiteLLM runs more Python than the default step budget allows.
BUDGET=2000000000

equiv review w.py w.py temperature_one 'str[in MODELS]' \
  --reference-python "$OLD" --candidate-python "$NEW" --versions-of litellm \
  --budget "$BUDGET" --all > rerun-temperature_one.txt 2>&1 || true
equiv review w.py w.py tokens str \
  --reference-python "$OLD" --candidate-python "$NEW" --versions-of litellm,tiktoken \
  --budget "$BUDGET" > rerun-tokens.txt 2>&1 || true

echo "Compare with: diff <(grep receipt-id out-temperature_one.txt) <(grep receipt-id rerun-temperature_one.txt)"
echo "              diff <(grep receipt-id out-tokens.txt) <(grep receipt-id rerun-tokens.txt)"
