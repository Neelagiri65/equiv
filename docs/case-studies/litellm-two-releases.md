# Case study: LiteLLM, one release against the next

Most of what `equiv` does is compare two versions of a function in the same
project. This one is different. I wanted to know whether a new release of a
library still behaves like the last one, from the point of view of code that
calls it.

[LiteLLM](https://github.com/BerriAI/litellm) was a good candidate. Version
1.110 moved a lot of its internals to Rust, and 1.109.1 was the last release
before that. If anything a caller depends on had shifted, this was the place to
look.

Everything below was produced by `equiv` v0.32.0 and can be re-run:
[`examples/case-studies/litellm-two-releases/`](../../examples/case-studies/litellm-two-releases/).

## The setup

Two virtual environments, one with `litellm==1.109.1` and one with
`litellm==1.110.0rc2`. One small file, [`w.py`](../../examples/case-studies/litellm-two-releases/w.py),
that calls LiteLLM the way an application would. Every call is mocked, so
nothing is sent to any provider.

`equiv` runs that same file twice, once in each environment, on the same
inputs:

```
equiv review w.py w.py temperature_one 'str[in MODELS]' \
  --reference-python old/bin/python --candidate-python new/bin/python \
  --versions-of litellm
```

`--reference-python` and `--candidate-python` say which interpreter runs each
side. `--versions-of litellm` asks each side to report which LiteLLM it
actually imported. That report comes from inside the process that ran the code,
and it goes into the receipt.

## Finding: `temperature=1` stopped working for four of six model names

`temperature_one(model)` makes one mocked completion call with `temperature=1`
and returns `"ok"` or the name of the exception.

```
COUNTEREXAMPLE at input ('claude-sonnet-5-5')
  candidate -> 'UnsupportedParamsError'
  reference -> 'ok'
disagreements: 4 of 6 judged inputs
  at ('claude-sonnet-5-5'): candidate -> 'UnsupportedParamsError', reference -> 'ok'
  at ('anthropic/claude-opus-4-8'): candidate -> 'UnsupportedParamsError', reference -> 'ok'
  at ('vertex_ai/claude-sonnet-5-5'): candidate -> 'UnsupportedParamsError', reference -> 'ok'
  at ('azure_ai/claude-opus-4-8'): candidate -> 'UnsupportedParamsError', reference -> 'ok'
reference: python 3.14.9, litellm 1.109.1
candidate: python 3.14.9, litellm 1.110.0rc2
```

These models only accept a temperature of 1. In 1.109.1, passing 1 works and
any other value is refused with a clear message. In 1.110.0rc2, passing 1 is
refused too, on the Anthropic, Vertex and Azure routes. The Bedrock name and
the older model in the list behave the same in both releases.

This was not caused by the Rust work. It came from a Python change in the same
release. `equiv` does not know or care where a difference comes from. It
compares what the caller sees.

I reported it as [BerriAI/litellm#51394](https://github.com/BerriAI/litellm/issues/51394).
A contributor confirmed the cause and opened a fix the same day
([#51398](https://github.com/BerriAI/litellm/pull/51398)). At the time of
writing the fix is open and waiting for review.

Full output: [`out-temperature_one.txt`](../../examples/case-studies/litellm-two-releases/out-temperature_one.txt).

## A pass, for contrast: token counts did not change

`tokens(text)` counts the tokens in a string for `gpt-4o`.

```
EQUIVALENT: agreed on all 1717 generated inputs (seed 1)
reference: python 3.14.9, litellm 1.109.1, tiktoken 0.14.0
candidate: python 3.14.9, litellm 1.110.0rc2, tiktoken 0.14.0
tiktoken: same version on both sides (0.14.0); the version does not tell the two installations apart
```

1,717 generated strings, the same count from both releases every time. That is
a statement about those inputs and nothing more.

Full output: [`out-tokens.txt`](../../examples/case-studies/litellm-two-releases/out-tokens.txt).

## Why the two `reference:` and `candidate:` lines matter

Before v0.32.0 I did this comparison by hand, with a line at the top of each
file that pushed a different folder onto Python's import path. It worked, but
nothing in the result said which LiteLLM each side had really used. When I
wanted to be sure, it took a day of extra checks to prove it.

Now each side says what it ran. If the package you name is not installed on a
side, was never imported, or was loaded from somewhere other than that
environment, the result is "could not check" and there is no receipt. A receipt
cannot name a version that did not run.

I tried the old trick again to see what would happen: a file that redirects the
import to the other release's folder. The candidate line came back as
`litellm 1.109.1`, which is what had actually run on that side.

## What this does not show

- It is a test on generated inputs. A pass means the two releases agreed on
  those inputs.
- It covers the two functions in `w.py`. It says nothing about the rest of
  LiteLLM.
- The two environments were built with `pip install`, so everything else in
  them (dependencies, Python itself) was whatever pip resolved on the day.
  `--versions-of` records only the packages you name.
- I ran it on macOS. I have not run it on Linux or Windows.

## Re-running it

```
cd examples/case-studies/litellm-two-releases
./run.sh
```

It needs `equiv` 0.32.0 or later on PATH, `python3` with the `venv` module, and
network access for the two installs. It builds both environments under
`.work/`, runs the two reviews and writes `rerun-*.txt`. The receipt ids should
match the saved outputs.
