#!/usr/bin/env bash
# Reject AI/assistant attribution trailers in commit messages.
# Policy: CLAUDE.md / docs/dev/commit-style.md — provenance belongs in the PR
# thread and git's own metadata, never stamped into the permanent commit log.
# Usage: check_commit_trailers.sh <commit-msg-file>
set -euo pipefail
msg_file="${1:?usage: check_commit_trailers.sh <commit-msg-file>}"

# Trailer-specific, anchored patterns so prose that *mentions* the policy
# (e.g. this very commit) does not trip the gate.
patterns=(
  '^[[:space:]]*Claude-Session:'
  '^[[:space:]]*Co-Authored-By:.*[Cc]laude'
  '^[[:space:]]*Generated with Claude'
  'claude\.ai/code/session_'
  '🤖[[:space:]]*Generated'
)
for pat in "${patterns[@]}"; do
  if grep -qE "$pat" "$msg_file"; then
    echo "commit-msg REJECTED — AI/assistant attribution trailer found:" >&2
    grep -nE "$pat" "$msg_file" | sed 's/^/    /' >&2
    echo "Strip it before committing. Provenance goes in the PR, not the log." >&2
    echo "(policy: CLAUDE.md)" >&2
    exit 1
  fi
done
