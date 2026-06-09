# Golden IR format silently reverted compact→pretty by an auto-format sweep

Date: 2026-06-09
Project: camdl
Tags: goldens, ci, serialization, process, agent-guardrails
Status: root-caused; remediation in progress
Class: code-vs-code (committed goldens drifted from compiler output) + process

## Summary

The CI `test` job's "Golden files unchanged" step went red: `make update-golden`
regenerated all 37 `ocaml/golden/*.ir.json` and `git diff --exit-code` found
drift against the committed files. The drift is **whitespace-only** — IR content
is byte-identical after normalization, no model compiles differently — but it is
a real format regression: the committed goldens are *pretty-printed* while the
compiler emits *compact* (one element per line, the format chosen in `bf5d13b`).

The regression was introduced **34 days before it was detected**, by a commit
whose message describes only a documentation proposal. It survived because the
CI gate that exists to catch exactly this was masked for that window by
unrelated upstream failures, and because the change was swept into the commit
invisibly (not in the message, not — apparently — reviewed in `--stat`).

## Timeline (verified)

Line count of `ocaml/golden/seir_observations.ir.json` (~220 = pretty, ~26 =
compact), `git log --format=%h -- <file>` then `git show <c>:<file> | wc -l`:

```
... 2026-03 → 2026-05-30   ~280 lines   pretty (original)
bf5d13b  2026-05-30   26 lines   feat(ir)!: compact IR JSON serialization — 4.6x faster, 5x smaller
298494b  2026-06-04   26 lines   fix(dsl): incidence() over a stratified transition family
93b2de5  2026-06-05  220 lines   docs(dev): proposal — observation-data binding (gh#171)   ← reverted
... HEAD             220 lines   pretty (drifted from compiler since 2026-06-05)
```

- **`bf5d13b` (2026-05-30) — the deliberate switch to compact.** A `feat(ir)!`
  with a proposal (`docs/dev/proposals/archive/post-alpha/2026-05-30-compact-ir-serialization.md`)
  and benchmarks: on the Kano national model, pretty-printing was ~97% of
  compile time and ~80% of IR bytes; compact cut compile 20.4s→4.4s (4.6×) and
  IR 1814.6MB→360.9MB (5.0×). It kept `camdlc --pretty`/`inspect` for human
  inspection and a `canonical_equiv` test proving compact and pretty encode
  identical content. A well-reasoned, documented, benchmarked decision.
- **`93b2de5` (2026-06-05) — the accidental revert.** Titled "docs(dev):
  proposal — observation-data binding (gh#171)". Its message is entirely about
  the proposal and **mentions nothing about goldens, format, or whitespace**
  (`git log -1 --format=%B 93b2de5 | grep -iE 'golden|ir.json|pretty|compact'`
  → no matches). But its diff touched **48 golden `.ir.json` files (+22047
  −1892)** plus a markdown table reflow across `CLAUDE.md`, `README.md`, every
  spec, all the incident docs. It is a **global cosmetic formatter pass**
  (markdown reflow + JSON pretty-print; 0 CRLF changes) committed under a
  proposal message — almost certainly an editor/tool reformat-on-save captured
  by a broad `git add`.

## Detection

The golden step runs *after* "Install OCaml deps" → "Clippy" → "Rust tests" in
`ci.yml`. Those earlier steps had been failing for the whole window — first an
OCaml-build break (`alcotest` not installed; `ci.yml` dropped `--with-test` in
the CI-split commit), then a `runid` reclaim concurrency race. The job
fail-fasts, so the golden step **never ran** and its red was invisible. Only
after both upstream failures were fixed (this session) did the golden step
execute on `651a275` and surface the 34-day-old drift.

## Root cause and compounding failures

The drift itself is benign (whitespace; `bf5d13b`'s compact is the intended
format). The *process* is the incident — five quiet failures in a row:

1. **A formatter silently rewrote load-bearing fixtures** (`*.ir.json`) in the
   working tree.
2. **A broad `git add` swept them into an unrelated commit** — "one concern per
   commit" violated, invisibly. The message described only the proposal, so
   message-only review saw nothing.
3. **The CI golden gate was maskable** — hidden behind upstream-step failures
   for 34 days; a required check that was never actually reaching the golden
   step looked no worse than one that passed it.
4. **The guardrail meant to prevent this was inert three ways.** The rule
   "golden-regen commits … are explicit human-loop changes" lived **only in
   `AGENTS.md`**, which **Claude Code does not load** (it loads `CLAUDE.md`; no
   `@import` of `AGENTS.md` existed). It **cited `CLAUDE.md`** for a rule
   `CLAUDE.md` did not actually contain (a phantom citation). And `git blame`
   shows that line was **inserted by `93b2de5` itself** — the guardrail was born
   in the same commit that violated it.
5. **The change is unattributable.** Per `docs/dev/commit-style.md` rule #1
   ("No AI / tooling trailers. Ever."), agent and human commits are byte-
   identical; author and committer on `93b2de5` are both the maintainer, which
   is the default for any commit in the repo. Whether an agent or a human ran
   the commit cannot be recovered from git.

## Impact

- IR content unchanged (verified: JSON-normalized committed == compiler output
  across all 37 drifted goldens — 37 whitespace-only, 0 content-changed). No
  model simulates or fits differently; no posterior affected.
- The committed goldens were 5× larger than intended and the CI golden gate was
  red once unmasked.

## Remediation

Done / in this change:

- **Regenerate `ocaml/golden/` to compact** (`make update-golden`) — restores
  `bf5d13b`'s intended format and greens the gate. Pure format restoration
  (content verified identical).
- **Move the rule into `CLAUDE.md`** ("Goldens are an explicit, reviewed,
  human-loop change — never collateral"), where Claude actually reads it; the
  phantom `AGENTS.md`→`CLAUDE.md` citation is removed.
- **Split agent guidance by audience** — `AGENTS.md` (a *modeler*-facing file
  that had accreted dev rules and sat unread by Claude at the repo root) moved
  to `docs/agents.md` and served via `camdl docs agents`; the root `AGENTS.md`
  is now a thin signpost. Modeler guidance and developer guidance no longer
  share a file.

Recommended follow-ups (not in this change):

- **`.gitattributes`** marking `*.ir.json` as generated, and/or a pre-commit or
  CI lint that **fails a golden change not matching `camdlc` output** — so a
  stray reformat fails loudly at the source instead of riding into a commit.
- **Un-mask the golden gate** — run the golden check early or as its own job so
  it cannot hide behind upstream-step failures.

## Verification

```
# format history
$ for c in bf5d13b 298494b 93b2de5; do git show $c:ocaml/golden/seir_observations.ir.json | wc -l; done
26
26
220

# 93b2de5 mentions nothing about goldens
$ git log -1 --format=%B 93b2de5 | grep -iE 'golden|ir\.json|pretty|compact|whitespace'
(no output)

# the guardrail line was inserted by 93b2de5 itself
$ git blame -L '/explicit human-loop/,+1' -- AGENTS.md
93b2de59 (Vince Buffalo 2026-06-05 11:55:45 -0700 83)   - golden-regen commits. Per CLAUDE.md, these are explicit human-loop changes.

# CLAUDE.md never imported AGENTS.md and had no such rule
$ grep -niE 'AGENTS|@import|human-loop|consent' CLAUDE.md
(no matches)

# drift is whitespace-only (content identical) across all 37 goldens
# JSON-normalized committed == regenerated → 37 whitespace-only, 0 content-changed
```
