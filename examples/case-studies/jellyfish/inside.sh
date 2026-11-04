#!/bin/bash
# Runs inside python:3.12 with --network none. $1 = jellyfish version.
set -u
pip install --no-index --find-links /wheels "jellyfish==$1" -q --root-user-action=ignore 2>/dev/null
export EQUIV_METRICS=off
cd /w
E=/equiv/equiv
echo "### jellyfish $1 / equiv 0.14.0"
python3 -c "import jellyfish._rustyfish as r, jellyfish._jellyfish as p; print('metaphone(high): rust', r.metaphone('high'), 'python', p.metaphone('high'))"
echo "## A. all inputs, default size, seed 1"
for f in soundex metaphone nysiis match_rating_codex; do echo "== $f"; $E review cand.py ref.py $f str --all 2>&1; echo "exit=$?"; done
for f in jaro_similarity jaro_winkler_similarity levenshtein_distance damerau_levenshtein_distance hamming_distance match_rating_comparison; do echo "== $f"; $E review cand.py ref.py $f str,str --all 2>&1; echo "exit=$?"; done
echo "## B. ASCII letters and spaces only (same guard both sides)"
for f in metaphone match_rating_codex; do for seed in 1 3 5; do echo "== $f seed $seed"; $E review gcand.py gref.py $f str 2000 $seed --all 2>&1; echo "exit=$?"; done; done
echo "## C. metaphone word-final 'gh' disagreements by input count and seed"
for n in 200 500 1000 2000; do for seed in 1 3 5 7 9; do c=$($E review cand.py ref.py metaphone str $n $seed --all 2>&1 | grep -cE "gh'\)"); echo -n "$c "; done; echo " <- n=$n seeds 1 3 5 7 9"; done
