#!/usr/bin/env bash
# check_cli_docs.sh — CLI run-gate for camdl docs.
#
# Extracts every `camdl …` invocation from fenced ```bash blocks in a doc,
# runs each in a throwaway temp dir against the freshly-built worktree binary,
# and classifies the outcome:
#
#   DRIFT    — the command surface itself is wrong: clap rejected an unknown
#              subcommand or an unrecognized flag/argument (exit 2 + a clap
#              "error: ..." about the surface). The doc references a
#              command/flag that does NOT exist. HIGH-VALUE finding.
#   EXPECTED — surface parsed fine; failure is about runtime inputs (missing
#              file, model path that doesn't exist, missing required value).
#   RAN      — exit 0.
#
# The crux: clap parse-layer rejection (exit 2, "unexpected argument" /
# "unrecognized subcommand" / "invalid value") == DRIFT. Anything the
# command's own runtime emitted (it parsed, then tried to open a file) ==
# EXPECTED. clap's exit code does the heavy lifting because camdl uses a
# *typed* clap parser, not a hand-rolled one.
#
# Usage: check_cli_docs.sh [DOC.md ...]   (defaults to docs/workflow.md)

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CAMDL="$REPO/rust/target/release/camdl"
CAMDLC_DIR="$REPO/ocaml/_build/default/bin"

if [[ ! -x "$CAMDL" ]]; then
  echo "FATAL: $CAMDL not built. Run: make build-rust" >&2
  exit 1
fi

# Prepend the worktree binaries so `camdl`'s camdlc delegation resolves to
# the worktree compiler (camdlc.exe), shadowing any stale ~/.local/bin copy.
WORKBIN="$(mktemp -d)"
ln -sf "$CAMDL" "$WORKBIN/camdl"
[[ -x "$CAMDLC_DIR/camdlc.exe" ]] && ln -sf "$CAMDLC_DIR/camdlc.exe" "$WORKBIN/camdlc"
export PATH="$WORKBIN:$PATH"

# Portable timeout: prefer `timeout`, fall back to `gtimeout` (coreutils on
# macOS), else run without a cap.
TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then TIMEOUT="timeout 20"
elif command -v gtimeout >/dev/null 2>&1; then TIMEOUT="gtimeout 20"; fi

DOCS=("$@")
[[ ${#DOCS[@]} -eq 0 ]] && DOCS=("$REPO/docs/workflow.md")

n_drift=0 n_expected=0 n_ran=0 n_skip=0

classify() {
  # $1 = exit code, $2 = stderr file
  local code="$1" errf="$2"
  local err; err="$(cat "$errf")"
  # clap parse errors exit 2 with a leading "error:" line naming the surface
  # problem. These patterns are clap's own wording (clap 4.x).
  if grep -qiE 'unexpected argument|unrecognized subcommand|invalid value|unknown (flag|option|argument)|the subcommand .* wasn.t recognized|tip: a similar (argument|subcommand) exists|which wasn.t expected' "$errf"; then
    echo "DRIFT"; return
  fi
  # Input-layer errors (file/dir not found, unreadable, missing data) are
  # EXPECTED regardless of exit code. This catches:
  #   - camdl runtime  : "cannot read X: No such file or directory"
  #   - camdlc (OCaml) : Sys_error("X: No such file or directory")  (exit 2)
  #   - fit summary    : "no such fit directory: X"
  #   - compare        : "no prequential.json at 'X'"
  #   - missing runtime-required value (e.g. "--rw-sd required")
  if grep -qiE 'no such file or directory|sys_error|no such (fit )?directory|cannot read|not found|no prequential|required \(e\.g\.|version mismatch' "$errf"; then
    echo "EXPECTED"; return
  fi
  # clap "required arguments were not provided": a missing required positional
  # is EXPECTED (user didn't supply the model/file). An unknown flag would
  # already be caught above.
  if grep -qiE 'required arguments were not provided|the following required' "$errf"; then
    echo "EXPECTED"; return
  fi
  # exit 2 with no recognized clap-surface phrase and no input-layer phrase:
  # surface this for review — usually a clap value-parse error worth seeing.
  if [[ "$code" == "2" ]]; then
    echo "DRIFT?"; return
  fi
  if [[ "$code" == "0" ]]; then echo "RAN"; return; fi
  # nonzero, runtime: missing file, bad model, etc. == EXPECTED
  echo "EXPECTED"
}

run_one() {
  local line="$1" cmd="$2"
  local tmp; tmp="$(mktemp -d)"
  local errf; errf="$tmp/stderr"
  # Run the command with `camdl` resolving to the worktree binary. eval is
  # needed because doc commands embed quotes/backslashes. Cap runtime so a
  # command that *does* parse and start a long fit can't hang the gate.
  # Doc commands embed angle-bracket PLACEHOLDERS (`<fit-dir>`, `<fields>`)
  # that the prose tells the user to substitute. They are not shell syntax;
  # rewrite each `<token>` to a literal nonexistent path so the *surface*
  # still parses (positional consumed) and the command fails at the
  # input layer (EXPECTED), not in our eval.
  local safe; safe="$(echo "$cmd" | sed -E 's/<[a-zA-Z0-9_.-]+>/__PLACEHOLDER__/g')"
  local code
  ( cd "$tmp" && eval "$TIMEOUT $safe" >/dev/null 2>"$errf" ); code=$?
  [[ $code == 124 ]] && { echo "  [line $line] TIMEOUT (parsed+started) -> EXPECTED: $cmd"; ((n_expected++)); rm -rf "$tmp"; return; }
  local verdict; verdict="$(classify "$code" "$errf")"
  local snippet; snippet="$(head -3 "$errf" | tr '\n' '|' | cut -c1-200)"
  printf '  [line %s] %-9s (exit %s): %s\n' "$line" "$verdict" "$code" "$cmd"
  [[ -n "$snippet" ]] && printf '             stderr: %s\n' "$snippet"
  case "$verdict" in
    DRIFT)    ((n_drift++));;
    DRIFT\?)  ((n_drift++));;
    EXPECTED) ((n_expected++));;
    RAN)      ((n_ran++));;
  esac
  rm -rf "$tmp"
}

# --selftest: prove the classifier separates synthetic DRIFT from EXPECTED.
if [[ "${1:-}" == "--selftest" ]]; then
  echo "===== SELF-TEST ====="
  fail=0
  check_expect() { # $1 expected verdict, $2 command
    local tmp errf code v
    tmp="$(mktemp -d)"; errf="$tmp/e"
    ( cd "$tmp" && eval "$2" >/dev/null 2>"$errf" ); code=$?
    v="$(classify "$code" "$errf")"
    if [[ "$v" == "$1" ]]; then printf '  ok   %-9s %s\n' "$v" "$2"
    else printf '  FAIL want=%s got=%s : %s\n     %s\n' "$1" "$v" "$2" "$(head -1 "$errf")"; fail=1; fi
    rm -rf "$tmp"
  }
  check_expect DRIFT    "camdl frobnicate foo"
  check_expect DRIFT    "camdl simulate m.camdl --not-a-flag"
  check_expect DRIFT    "camdl fit bogus fit.toml"
  check_expect EXPECTED "camdl simulate /no/such/model.camdl --seed 1"
  check_expect EXPECTED "camdl fit run /no/such/fit.toml"
  check_expect EXPECTED "camdl fit summary /no/such/dir"
  [[ $fail -eq 0 ]] && echo "self-test PASS" || echo "self-test FAIL"
  exit $fail
fi

for doc in "${DOCS[@]}"; do
  echo "===== $doc ====="
  # Extract lines inside ```bash fences, join backslash-continuations, keep
  # only lines that *start* a `camdl ` invocation (ignore shell comments).
  awk '
    /^```bash[[:space:]]*$/ {inblk=1; next}
    /^```/ {inblk=0; next}
    inblk {print NR"\t"$0}
  ' "$doc" > /tmp/_cli_lines.tsv

  # Reassemble continuations: a line ending in backslash continues.
  acc=""; accln=""
  while IFS=$'\t' read -r ln text; do
    # strip comments-only lines
    case "$text" in \#*) continue;; esac
    if [[ -n "$acc" ]]; then
      acc="$acc ${text%\\}"
    else
      accln="$ln"; acc="${text%\\}"
    fi
    if [[ "$text" == *\\ ]]; then continue; fi
    # acc is a full logical line now
    trimmed="$(echo "$acc" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
    case "$trimmed" in
      camdl\ *|camdl) run_one "$accln" "$trimmed";;
      "") : ;;
      *) : ;;  # non-camdl shell line (watch, etc.) — skip
    esac
    acc=""; accln=""
  done < /tmp/_cli_lines.tsv
done

echo
echo "===== SUMMARY ====="
echo "DRIFT (FAIL):    $n_drift"
echo "EXPECTED (OK):   $n_expected"
echo "RAN (OK):        $n_ran"
[[ $n_drift -gt 0 ]] && exit 1
exit 0
