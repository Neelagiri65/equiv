# equiv plugin for Claude Code

Configuration only. It registers the two things `equiv init claude-code` writes
into a project, for every project you open:

- the MCP server `equiv mcp`, which gives the agent the tools `review` and `check`;
- the `PostToolUse` hook `equiv hook claude-code`, which runs after every Edit,
  Write or MultiEdit of a tracked `.py` file and tells the agent whether the
  functions that changed still behave as they did at `HEAD`.

What they do, and the two rules they keep (an agent cannot narrow a check; types
an agent supplies are marked), is in [docs/agents.md](../../docs/agents.md).
Both versions of a function are run locally with python3, as you. This is not a
sandbox.

## It needs the equiv binary

The plugin does not contain equiv and does not download it. Install version
0.30.0 or later and make sure `equiv` is on PATH:

```
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Neelagiri65/equiv/releases/latest/download/equiv-cli-installer.sh | sh
```

Without the binary (or with one older than 0.30.0) nothing is checked: the
`equiv` server shows as failed under `/mcp`, and the after-edit hook does
nothing. The plugin says so. At the start of each session it looks for `equiv`
on the PATH Claude Code uses; if it is missing, or does not answer as equiv
0.30.0 or later, the agent is told that nothing is being checked and to tell
you, and it is told not to install anything itself. When the binary is fine the
check prints nothing.

After you install or update the binary:

- the after-edit hook works from the next edit;
- the server can stay failed for up to 15 minutes, because Claude Code remembers
  a failed connection for that long (reconnect it from `/mcp` to retry sooner);
- the notice the agent got at the start of the session is not withdrawn; it is
  gone in the next session.

The check is a line of `sh`. On Windows it runs where Claude Code runs hooks in
Git Bash. Without Git Bash, Claude Code uses PowerShell, where the check is
expected to fail with a hook error at session start and say nothing to the
agent; this has not been tested on Windows. The after-edit hook and the server
do not depend on it.

## Install

```
/plugin marketplace add Neelagiri65/equiv
/plugin install equiv@equiv
```

## With `equiv init claude-code` in the same project

A project that already has the hook from `equiv init claude-code` ends up with
two copies of it once the plugin is installed, and Claude Code runs both. From
equiv 0.31.0 an edit is still checked once: the first copy to take the tool
call reports and the other stays silent. With 0.30.0 every edit is checked
twice. Claude Code keeps one copy of the server.

To have only one: uninstall the plugin, or undo `init` (remove `equiv` from
`mcpServers` in `.mcp.json` and the `PostToolUse` entry whose command is
`equiv hook claude-code` from `.claude/settings.json`).

The tool names differ between the two: `mcp__plugin_equiv_equiv__review` from
the plugin, `mcp__equiv__review` from `init`.

## A paragraph for your AGENTS.md

The plugin does not write this. Add it if you want the agent to check its own
refactors without being asked:

> After rewriting, refactoring or porting a function, check that its behaviour
> is unchanged with the equiv MCP tool `review` (or `equiv review`), and include
> the result in your summary. If equiv reports a difference, fix it or say the
> change is intended. If it could not check, say so. A pass covers behaviour on
> generated inputs only, not intent.

## Remove

```
/plugin uninstall equiv@equiv
```
