# Proposals

Design proposals for camdl language and engine changes.

## Format

```
docs/proposals/YYYY-MM-DD-proposal-slug.md
```

### Header block

Every proposal starts with a metadata block:

```markdown
# Proposal: short title

**Status:** Proposal | Accepted | Implemented | Superseded **Date:** YYYY-MM-DD
**Implemented:** commit `abc1234`, YYYY-MM-DD (added when merged) **Superseded
by:** docs/proposals/YYYY-MM-DD-newer.md (if applicable) **Motivation:**
One-sentence summary of why this exists.
```

Update the status and implementation fields as the proposal progresses. Once
implemented, the proposal becomes a historical record of the design rationale —
don't delete it.

## Naming

Use the date the proposal was first written, not the implementation date. The
slug should be descriptive enough to find by scanning filenames: `events-block`,
`balance-compartment`, `cooling-schedule`.

## Archive (epochs)

The top-level `proposals/` directory holds the **currently-live** set: proposals
whose Status is `Proposal`, `Accepted`, `Deferred`, or otherwise still shaping
unlanded work. Once a proposal is fully `Implemented` or `Superseded` — its
design question is closed — move it into an epoch subdirectory so the live set
stays scannable (the "read the relevant proposals first" rule has a read cost
proportional to how many live-looking docs there are):

```
docs/dev/proposals/archive/<epoch>/YYYY-MM-DD-slug.md
```

Epochs bucket by the release line the proposal closed under, split at release
tags. Bucket by the proposal's **file date** (its naming date), which is stable
and doesn't require chasing the implementing commit:

- `archive/pre-alpha/` — closed before the `v0.1.0-alpha` tag (2026-05-15).
- `archive/post-alpha/` — closed during the current 0.1 development cycle (dated
  on/after the alpha tag, before the 0.1.0 release).
- future: `archive/0.1/`, `archive/0.2/`, … one per release line as it is cut.

`Deferred` proposals (parked but unbuilt — an open question) stay in the live
set; only `Implemented`/`Superseded` get archived.

Moving a proposal to the archive is a pure `git mv` — its Status header already
records `Implemented: commit <sha>` / `Superseded by:`, so the archive is
physical tidying, not a content change. Do **not** archive a proposal that is
still `Proposal`/`Accepted` or that has unlanded follow-ups; an archived
proposal asserts "this epoch's design question is closed."

### Partially-implemented proposals

A proposal whose core landed but which still has unbuilt pieces is the common
messy case — and the worst kind of ambient context, because it reads as live but
is mostly history. Don't leave it half-live in `proposals/`. Resolve it one of
two ways:

- **Remaining work is ignorable** (won't be built, or was folded elsewhere):
  record it in the Status block —
  `Implemented; remaining
  <X> dropped — <reason>` — and archive the whole
  proposal. The design question is closed.
- **Remaining work is real**: split the unbuilt part into a fresh,
  narrowly-scoped proposal (with a `Split from:` pointer back), keep _that_ in
  the live set, mark the original `Implemented` and archive it. The live set
  then describes only work that is actually still open.

Either way, no file in the live `proposals/` set is partially done — each is
either fully open (`Proposal`/`Accepted`) or a clean remainder split.

### Release tags / versioning (for reference)

- **Git release tags** are annotated, semver, `vX.Y.Z[-pre]`, tracking the Rust
  workspace version in `rust/Cargo.toml`. First tag: `v0.1.0-alpha` (alpha,
  breaking changes still expected). Bump the Cargo version at each release so
  the tag and the crate agree.
- **`ir/VERSION`** (the OCaml↔Rust IR schema contract) is **independent** of the
  release version — it bumps only on a schema-breaking IR change, per the
  "Changing the IR schema" procedure. Currently `0.7`.
