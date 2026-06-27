#!/usr/bin/env bash
# Garbage-collect stale hash-named camdlc compilers (`camdlc-<git-hash>`) from an
# install dir.
#
# Every `make install` / `make install-camdlc` drops a `camdlc-<hash>` beside the
# plain `camdlc` and NEVER removes the previous one, so over many commits they
# pile up (hundreds of MB). Only ONE is ever reachable: the `camdlc-<hash>` whose
# hash matches the installed `camdl` (find_camdlc rule 1a, `rust/crates/cli/
# src/util.rs` — the running camdl looks in its own dir for `camdlc-<its hash>`).
# The plain `camdlc` covers everything else. So all the other `camdlc-<hash>`
# copies are dead weight and safe to remove (recoverable: `git checkout <c> &&
# make install` re-creates any hash).
#
# Policy: keep the live hash (the installed `camdl`'s own) + the N most-recent as
# a buffer; delete the rest. Idempotent.
#
# Usage: gc_camdlc.sh [--dir DIR] [--keep N] [--dry-run]
#   --dir DIR    install dir to clean (default: $INSTALL_DIR or ~/.local/bin)
#   --keep N     most-recent hash-named copies to retain as a buffer (default: 5)
#   --dry-run    list what would be removed; delete nothing
set -eu

dir="${INSTALL_DIR:-$HOME/.local/bin}"
keep=5
dry=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dir)     dir="$2"; shift 2 ;;
    --keep)    keep="$2"; shift 2 ;;
    --dry-run) dry=1; shift ;;
    *) echo "gc_camdlc: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

# The live hash = whatever `camdl` is installed in $dir resolves to. find_camdlc
# rule 1a wants `camdlc-<that hash>`; `camdl --version` prints "...+<hash> (...)".
live=""
if [ -x "$dir/camdl" ]; then
  live="$("$dir/camdl" --version 2>/dev/null \
          | sed -n 's/.*+\([0-9a-f][0-9a-f]*\).*/\1/p' | head -1)"
fi

# Hash-named copies only (never the plain `camdlc`/`camdl`), newest first.
# Filenames are `camdlc-<hex>` (no spaces), so word-splitting the list is safe.
all="$(ls -t "$dir"/camdlc-* 2>/dev/null || true)"
if [ -z "$all" ]; then
  echo "gc_camdlc: no camdlc-* in $dir — nothing to do"
  exit 0
fi

# Candidates for deletion: every hash-named copy except the newest $keep.
candidates="$(printf '%s\n' "$all" | tail -n "+$((keep + 1))")"

freed=0
ndel=0
for f in $candidates; do
  # Never delete the live hash, even if it falls outside the newest $keep.
  [ -n "$live" ] && [ "$f" = "$dir/camdlc-$live" ] && continue
  sz="$(stat -f%z "$f" 2>/dev/null || stat -c%s "$f" 2>/dev/null || echo 0)"
  freed=$((freed + sz))
  ndel=$((ndel + 1))
  if [ "$dry" -eq 1 ]; then
    echo "would remove $(basename "$f")"
  else
    rm -f "$f"
  fi
done

total="$(printf '%s\n' "$all" | grep -c .)"
kept=$((total - ndel))
human="$(awk -v b="$freed" 'BEGIN{
  u="B"; if(b>=1073741824){b/=1073741824;u="GB"}
  else if(b>=1048576){b/=1048576;u="MB"}
  else if(b>=1024){b/=1024;u="KB"}
  printf (u=="B"?"%d%s":"%.1f%s"), b, u }')"
verb="$([ "$dry" -eq 1 ] && echo 'would free' || echo 'freed')"
echo "gc_camdlc: kept $kept (live=${live:-none} + $keep newest), removed $ndel, $verb $human  [$dir]"
