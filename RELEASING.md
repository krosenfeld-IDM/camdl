# Releasing camdl

The operational runbook for cutting a release. [`VERSIONING.md`](VERSIONING.md)
is the *policy* — what a version number promises a user; this is the
*procedure*. Read the policy first.

## Tag conventions

- **Release tags:** `vMAJOR.MINOR.PATCH` — e.g. `v0.2.0`. Always **annotated**
  (`git tag -a`), so the tag carries a message and date; never lightweight.
- **Pre-release tags:** `vX.Y.Z-<stage>.<n>` with a **dot-numbered** counter —
  `v0.2.0-rc.1`, `v0.9.0-beta.2`, `v0.1.0-alpha.3`. The dotted counter is what
  lets SemVer tooling and git-cliff order and bump them mechanically.
- **Stage precedence** (SemVer): `alpha < beta < rc < release`.
- Tags are immutable once pushed. Never move or delete a published release tag —
  cut a new one.

### The current tag is `v0.1.0-alpha` — mind the bump gotcha

`v0.1.0-alpha` has no dotted counter, so `git-cliff --bumped-version` (and any
SemVer bumper) reads the next version as `v0.1.0-alpha.1` — it increments the
*pre-release*, not the minor. That is almost never what you want after a batch
of features. After alpha:

- Cut the next version **explicitly** as `v0.2.0` — a MINOR bump, the right call
  per the policy for features + breaking changes accumulated since alpha. Do not
  trust the auto-bump here.
- From then on, give every pre-release a dotted counter (`-beta.1`, `-rc.1`) so
  the bumper behaves.

### Backup tags don't belong in the release namespace

`git tag` currently also lists `cas-overhaul`, `pre-alpha-rerun`,
`progress-prerebase-backup` — working backups, not releases. `cliff.toml`
already ignores them for version detection (its `tag_pattern` matches only
`v<num>.<num>.<num>`), but they clutter `git tag` and completion. Delete a backup
tag once its branch has merged, or namespace future scratch tags as
`backup/<name>` so the `v*` space stays release-only.

## The alpha → beta → 1.0 ladder

Each rung is a promise, not a maturity badge (see the policy for the exact
surface each covers):

- **`0.x.0-alpha.*`** — surface moving freely; no compatibility promise. *We are
  here.*
- **`0.x.0-beta.*`** — the DSL + CLI + output-format surface is **substantially
  frozen** and the deprecation policy is in force: a surface element is removed
  only after a deprecation cycle that names its replacement. Cut beta to tell
  users "build against this; breaks arrive with warnings, not surprises."
- **`1.0.0`** — the surface is **stable**: breaking it requires a MAJOR bump. Cut
  `1.0` for *stable surface*, not for *feature complete*.

## Choosing the version

Derive the bump from the Conventional Commit types since the last release tag:

- any `feat`, or any `!` / `BREAKING CHANGE:` footer → **MINOR** (`0.x+1.0`)
  while pre-1.0 (MAJOR at ≥ 1.0).
- only `fix` / `perf` / `docs` / `refactor` / `test` / `ci` / `build` / `chore`
  → **PATCH** (`0.x.y+1`).

Inspect the window first: `git log --oneline <last-tag>..HEAD`. The mechanical
suggestion is `git-cliff --bumped-version` — override it per the alpha gotcha
above.

## Cutting a release — step by step

**Preconditions** (all must hold on the release commit):

- [ ] CI green — every workflow, not just `test`.
- [ ] `make test` passes locally (unit + golden + integration).
- [ ] Goldens clean: `make update-golden && git diff --exit-code ir/golden/ ocaml/golden/`.
- [ ] The book (camdl-book) renders against this commit.
- [ ] No `#[ignore]` / dead-code / `--no-verify` shortcuts introduced.
- [ ] Every breaking change since the last tag has an `old → new` entry in
      [`docs/language-changes.md`](docs/language-changes.md) and a migration line
      in the notes.

**Steps:**

1. **Decide `vX.Y.Z`** (see "Choosing the version").
2. **Bump every manifest so they agree:**
   - `rust/Cargo.toml` (workspace `version`)
   - each `rust/crates/*/Cargo.toml` (until they inherit — see "Setup
     improvements")
   - `ocaml/camdl.opam` (`version:`)

   Verify they match:
   ```
   grep -rn '^version' rust/Cargo.toml rust/crates/*/Cargo.toml
   grep '^version' ocaml/camdl.opam
   ```
   `ir/VERSION` is the **IR schema** version — bump it *only* if the IR contract
   itself changed, independently of the release version.
3. **Regenerate the changelog spine:** `make changelog` (writes `CHANGELOG.md`).
   git-cliff turns the `[unreleased]` section into `[X.Y.Z] - <date>` once the
   tag exists; review the grouping.
4. **Draft the notes** with the `/release-notes` skill: the git-cliff spine plus
   a narrative grouped by area, a migration step for every breaking change, and a
   *Formats & compatibility* section for any `ir/VERSION` or `fit.toml` change.
   The maintainer edits.
5. **Commit:** `chore(release): vX.Y.Z` — version bumps + `CHANGELOG.md`.
6. **Tag and push:**
   ```
   git tag -a vX.Y.Z -m "camdl vX.Y.Z"
   git push origin main --follow-tags
   ```
7. **Publish:**
   ```
   gh release create vX.Y.Z --title "camdl vX.Y.Z" --notes-file <notes.md>
   ```
   Add `--prerelease` for any `-alpha`/`-beta`/`-rc` tag.

## Formats & compatibility

The IR schema (`ir/VERSION`) and `fit.toml` are versioned separately from the
release number (policy §"What the version covers"). When either changes, surface
it under a **Formats & compatibility** heading in the notes — an `ir/VERSION`
bump means previously serialized `.ir.json` may not load, which is a user-visible
event even though it does not drive the release number.

## Setup improvements (as beta nears)

These reduce the chance of a botched release; none is in place yet.

- **Single-source the crate version.** Each of the seven `rust/crates/*/Cargo.toml`
  hardcodes its own `version`. Move to workspace inheritance
  (`[workspace.package] version = "…"` + `version.workspace = true` per crate) so
  a release bumps one line and the manifests can't drift.
- **A tag-triggered release workflow.** `release.yml` was removed while
  unfinished. Before beta, add a workflow that fires on `v*` tags, runs the full
  gate, and drafts the GitHub release from `CHANGELOG.md` — so step 7 is
  reproducible rather than manual.
- **Tag hygiene.** Delete merged backup tags; namespace future scratch tags under
  `backup/`.
