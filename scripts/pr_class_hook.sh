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

# Match only a command that actually runs the thing, not one that merely
# mentions it inside a quoted string or a heredoc.
if ! printf '%s' "$command_line" \
    | grep -qE '(^|[;&|]|&&)[[:space:]]*gh[[:space:]]+pr[[:space:]]+create([[:space:]]|$)'; then
    exit 0
fi

cd "$(git rev-parse --show-toplevel)"

# The hook runs in the session's working directory, which is not always the
# worktree holding the branch. When the command names its own refs, classify
# that pair rather than whatever branch is checked out here.
head=$(printf '%s' "$command_line" | sed -n 's/.*--head[= ]\([^ ]*\).*/\1/p')
base=$(printf '%s' "$command_line" | sed -n 's/.*--base[= ]\([^ ]*\).*/\1/p')
: "${base:=main}"

if [ -n "$head" ]; then
    changed=$(git diff --name-only "$(git merge-base "$base" "$head")...$head")
else
    changed=$(git diff --name-only "$(git merge-base "$base" HEAD)...HEAD")
fi

if ! printf '%s\n' "$changed" | grep -qE '^(mandible[^/]*/|xtask/src/|Cargo\.(toml|lock)$)'; then
    echo "direct to main: this diff touches no code" >&2
    echo "Nothing under mandible*/, xtask/src or Cargo.* changed." >&2
    echo "Commit it to main. See AGENTS.md, the pull-request class rule." >&2
    exit 2
fi

exit 0
