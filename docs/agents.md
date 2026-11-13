# equiv for coding agents

Three subcommands connect a coding agent to equiv: an MCP server the agent can
call, a hook that runs after each edit, and a command that writes the project
configuration for both. None of them decides anything. Every verdict they
report is one that `equiv review`, `equiv check` or `review-pr --auto` computed,
printed exactly as those commands print it.

Two rules hold for all three:

- **An agent cannot narrow a check.** It can choose what is compared, never how
  much of it is checked. The tools refuse input domains (`int[0..5]`,
  `str[ascii]`, `str[1]`, `str[in X]`), seeds, case counts, step budgets,
  `--errors-agree` and `--same-host`, and say why. A person can still use them
  with `equiv review` or a manifest.
- **Types an agent supplies are marked.** Argument types are read from the
  function's type hints. When a function has none, the agent may give a
  signature; the review then runs with `--types-hinted`, and the receipt tier
  says so (`tested-hinted`, `tested-same-host-hinted`, each followed by `-thin`
  when the evidence is thin). Wrong types make the result say nothing about the
  function's real inputs.

Both versions of a function are run locally with python3, as you, with your
files and network. **This is not a sandbox.** A pass covers behaviour on
generated inputs only, not intent, architecture, security or whole-program
correctness.

## Setup: `equiv init claude-code`

Run it in the project root:

```
equiv init claude-code
```

It adds the MCP server to `.mcp.json` and the hook to `.claude/settings.json`,
creating either file if needed. It never clobbers:

- every other key stays, in its order; a file is rewritten (2-space indentation)
  only when something is added to it, so a second run changes nothing and says so;
- an existing `equiv` server entry that runs something else is kept and reported;
- a hook whose command is `equiv hook claude-code` anywhere under `PostToolUse`
  is left alone;
- a file that is not valid JSON, or whose `mcpServers`, `hooks` or
  `hooks.PostToolUse` has the wrong type, stops the run with exit 3, and
  neither file is written.

It warns when `equiv` is not on PATH (Claude Code starts both commands by name),
and prints how to undo it and a paragraph for your `AGENTS.md` (it does not
write that file):

> After rewriting, refactoring or porting a function, check that its behaviour
> is unchanged with the equiv MCP tool `review` (or `equiv review`), and include
> the result in your summary. If equiv reports a difference, fix it or say the
> change is intended. If it could not check, say so. A pass covers behaviour on
> generated inputs only, not intent.

`init` takes the agent's name; `claude-code` is the only one in this version.
Any other name is a usage error (exit 3).

What it writes, in a project that had neither file:

`.mcp.json`

```json
{
  "mcpServers": {
    "equiv": {
      "command": "equiv",
      "args": [
        "mcp"
      ]
    }
  }
}
```

`.claude/settings.json`

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "equiv hook claude-code",
            "timeout": 300
          }
        ]
      }
    ]
  }
}
```

The server alone can also be added with
`claude mcp add --transport stdio --scope project equiv -- equiv mcp`. Claude
Code asks you to approve a project MCP server before its first use.

To undo: remove `equiv` from `mcpServers` in `.mcp.json`, and the `PostToolUse`
entry whose command is `equiv hook claude-code` from `.claude/settings.json`.

## `equiv mcp`

A Model Context Protocol server on stdin and stdout: one JSON-RPC 2.0 message
per line, nothing else on stdout, exit 0 when its input ends. It never sends a
request to the client.

Both protocol eras are served, chosen per request:

- **2024-11-05 to 2025-11-25**: `initialize` (the client's version is echoed if
  it is one of 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25; otherwise the
  answer is 2025-11-25), then `tools/list` and `tools/call`, which are also
  served without a prior `initialize`.
- **2026-07-28**: no handshake. A request that names this version in
  `params._meta["io.modelcontextprotocol/protocolVersion"]` gets results with
  `resultType: "complete"`, and `server/discover` lists the versions. A request
  naming another version gets error -32022 with the supported versions.

`ping` works in both. An unknown method is -32601, an unknown tool -32602, a
line that is not JSON -32700. Problems with a tool's inputs are tool results
with `isError: true` and a message saying what to change; nothing is run.

Relative paths are read from `$CLAUDE_PROJECT_DIR` when it is set (Claude Code
sets it), else from the server's working directory.

The server answers one request at a time. A review is bounded by its step
budget and safety limit, but `equiv check` has no limit on the work a call does
(see `docs/scope.md`), so an artifact that never returns holds the server until
the client stops it.

### Tool `review`

| input | |
|---|---|
| `candidate` (required) | the Python file with the new version |
| `function` (required) | the module-level function to compare |
| `reference` | a file with the original version |
| `base` | when `reference` is omitted: the git revision to read the candidate file from (default `HEAD`) |
| `signature` | argument types, only for a function without usable type hints: `int`, `str`, `float`, `bool`, `list`, `list[int]`, `list[str]`, `dict`, `dict[str,int]`, comma separated |

It runs `equiv review <candidate> <reference> <function> <types>`. Without
`reference`, the candidate file as committed at `base` is written to a private
temporary directory and used as the reference. The types come from the
function's hints, read by the same detector `review-pr --auto` uses. A
`signature` that contradicts the hints is refused, naming both. A function the
detector cannot type (no hints, or another reason it gives) needs a
`signature`, and the detector's reason is shown with the result. A function
that is identical in both versions is said to be identical; it can still be
reviewed with a `signature`, since something it calls may have changed, and the
result then says it compared the function with itself (an edit you already
committed is not covered: compare with the version before it). `base` given
beside `reference` is not used, and the result says so.

The result is what `equiv review` printed (stdout, then stderr), then:

```
exit code 1: differs (the input and both results are printed above)
types: read from the function's type hints
Scope: behavioural equivalence on generated inputs only, not intent, architecture, security or whole-program correctness. A pass means behaviour was preserved on the tested inputs, not that the change is correct.
```

Exit 0 is equivalent, 1 differs, 2 could not check (never a pass); all three
are normal results (`isError: false`). Exit 3, or a run that did not finish,
is `isError: true`.

### Tool `check`

| input | |
|---|---|
| `artifact` (required) | the `.wasm` file |
| `contract` (required) | the contract, `.eqct` text or `.eqc` bytes |
| `reference` | the reference `.wasm` a `refines` clause names by sha256 |

It runs `equiv check <artifact> <contract> [--reference <reference>]` and
returns the same way, with this line added: the contract was written by the
caller, so a pass means the artifact met that contract on the generated
inputs; read the contract before relying on it. The artifacts run in equiv's
built-in WebAssembly interpreter; a module that imports anything cannot run.

## `equiv hook claude-code`

A `PostToolUse` hook. It is advisory: it never blocks and always exits 0. It
runs after every Edit, Write or MultiEdit of a `.py` file that git tracks, and
costs a few seconds per changed function.

It compares the file on disk with its version at `HEAD`, finds the functions
that changed (the detector `review-pr --auto` uses) and checks them with the
checks `review-pr --auto` builds, at most six per edit. The per-function safety
limit is 30 seconds, or `$EQUIV_SAFETY_SECONDS` if that is lower. Inputs
harvested from the project's other files are not used, so a run costs the same
whatever the size of the repository; `equiv review-pr --auto --base HEAD` adds
them.

The agent is told about every changed function, never only the failures:

```
equiv compared the functions changed in src/m.py with the last commit (generated inputs only; behaviour, not intent):
`f`: differs at input (777): now returns 0, the committed version returns 1554. If this change is intended, say so; if it was meant to be a refactor, fix it.
`g`: same behaviour on 1697 generated inputs (receipt c85b31eb7dfc)
`h`: not checked: no supported type hint on `x` (int, str, float, bool, dict, list[int] or list[str]); add one to check it
`k`: not checked in this hook run; run `equiv review-pr --auto --base HEAD`
```

A thin-evidence or not-judged note follows a "same behaviour" line when the
receipt has one; a check that could not run says `could not check` with the
reason. When equiv cannot check the file at all (it no longer parses, python3
is missing, a bad `EQUIV_BUDGET`), the agent is told that instead.

It prints nothing when there is nothing to compare: the input is not valid
JSON or not a `PostToolUse` event, the tool input has no `file_path`, the file
is not `.py`, it is not in a git repository, it has no version at `HEAD` (new
or untracked), it is unchanged since `HEAD`, or no function in it changed.

An edit is checked once. Claude Code runs every registered copy of a hook, and
a project can have two copies of this one (from `equiv init claude-code` and
from the plugin). Both receive the same event, so the first copy to take the
tool call reports and any other copy of that tool call prints nothing. Copies
recognise each other by the event's `session_id` and `tool_use_id`, through a
file in a directory under the system temp directory that only you can write;
entries older than an hour are removed. An event without a `tool_use_id` is
always checked, and if that directory cannot be used the edit is checked by
every copy, as before.
