# LiteLLM two-releases case study: files

Write-up: [`docs/case-studies/litellm-two-releases.md`](../../../docs/case-studies/litellm-two-releases.md).

- `w.py`: the one file both sides run. `temperature_one` makes a mocked
  completion call with `temperature=1`; `tokens` counts the tokens in a string.
  Nothing is sent to a provider.
- `run.sh`: builds two virtual environments (`litellm==1.109.1` and
  `litellm==1.110.0rc2`) under `.work/` and runs both reviews with an
  interpreter per side. Needs `equiv` 0.32.0 or later on PATH, `python3` with
  `venv`, and network for the installs.
- `out-temperature_one.txt` / `out-tokens.txt`: the published output from
  equiv v0.32.0, with receipt ids. `run.sh` writes `rerun-*.txt`; the receipt
  ids should match.
