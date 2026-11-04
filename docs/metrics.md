# Metrics

Off by default. equiv sends nothing anywhere and writes no metrics file unless
you set `EQUIV_METRICS=on`. When on, `review-pr` appends one line of counts to
`./equiv-metrics.jsonl`: how many functions changed, how many were checked, the
verdicts and why the rest were not checked, plus the language, equiv version,
day and a hash of the git remote URL. The hash is unsalted, so anyone who knows
the URL can match it. Never code, function names, paths, values, receipt ids or
the repo's name in clear. `DO_NOT_TRACK=1` overrides.
