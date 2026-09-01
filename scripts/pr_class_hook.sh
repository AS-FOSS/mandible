#!/usr/bin/env bash
# PreToolUse hook body for `gh pr create`.
#
# Refuses to open a pull request when the branch's diff against `main` touches
# nothing under `mandible*/`, `xtask/src`, `corpus/` or the Cargo manifests.
# Prose, scripts, workflows and config go direct to main: a docs-only push
# skips CI entirely, so a self-opened self-merged pull request for a paragraph
# adds a merge commit and a dead branch while gating nothing.
#
# Exit 2 is the code the harness reads as "deny, and show the agent stderr".
#
# Install it by adding this to the PreToolUse hooks in `.claude/settings.json`,
# which is agent configuration and is not tracked here:
#
#   {
#     "matcher": "Bash",
#     "hooks": [
#       {
#         "type": "command",
#         "command": "scripts/pr_class_hook.sh"
#       }
#     ]
#   }
#
# The hook reads the tool call as JSON on stdin and only acts on a command
# that runs `gh pr create`.
set -euo pipefail

payload=$(cat)

command_line=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("tool_input",{}).get("command",""))' 2>/dev/null || echo "")

case "$command_line" in
    *"gh pr create"*) ;;
    *) exit 0 ;;
esac

cd "$(git rev-parse --show-toplevel)"

if [ "$(./scripts/pr_class.sh main)" = "direct" ]; then
    echo "direct to main: this diff touches no code" >&2
    echo "Nothing under mandible*/, xtask/src, corpus/ or Cargo.* changed." >&2
    echo "Commit it to main. See AGENTS.md, the pull-request class rule." >&2
    exit 2
fi

exit 0
