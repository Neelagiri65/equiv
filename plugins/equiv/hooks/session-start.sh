#!/bin/sh
# Says one line to the agent when the equiv binary is missing or too old to have
# the hook; says nothing when it is fine. Never fails the session.
if ! command -v equiv >/dev/null 2>&1; then
  echo 'equiv plugin: Claude Code cannot find the equiv command on its PATH, so nothing is being checked in this session: the after-edit comparison does not run and the equiv tools are unavailable. Tell the user once; do not install or update it yourself. Instructions: https://github.com/Neelagiri65/equiv#use-it-from-the-command-line'
elif ! equiv --help </dev/null 2>/dev/null | grep -q 'equiv hook'; then
  echo 'equiv plugin: the equiv command on PATH did not answer as equiv 0.30.0 or later (an older version, another program with that name, or one that cannot run), so nothing is being checked in this session. Tell the user once; do not install or update it yourself. Instructions: https://github.com/Neelagiri65/equiv#use-it-from-the-command-line'
fi
exit 0
