# Versioning policy

camdl follows [Semantic Versioning](https://semver.org/) over an explicitly
defined public surface. This document says what the version number promises, so
that "we bumped the minor" means something concrete to a user.

## What the version covers

The `camdl` / `camdlc` release version governs **user-facing behavior**:

- **The DSL** — grammar, keywords, dimensional rules, and the diagnostics
  contract (an error code is part of the surface; renumbering one is a change).
- **The CLI** — subcommands, flags, and their argument grammar.
- **Output and file formats** — trajectory/observation TSV columns, fit-directory
  layout, and other artifacts a user reads or scripts against.

Two surfaces are versioned **separately**, and their changes are surfaced in the
release notes under *Formats & compatibility* rather than driving the software
version:

- **The IR schema** — the OCaml↔Rust contract, versioned by `ir/VERSION`. A bump
  there means previously serialized `.ir.json` files may not load; it is a
  compatibility event independent of the user-facing version.
- **`fit.toml`** — the inference config format. Treated as part of the CLI surface
  for breaking-change purposes (a removed/renamed key is breaking).

What the version does **not** cover: internal crate/library APIs, the OCaml and
Rust module layout, test scaffolding, and anything under `docs/dev/`.

## SemVer, pre-1.0

While in `0.MINOR.PATCH` (the alpha/beta line):

- **MINOR** (`0.x.0`) — new features **and** breaking changes to the surface
  above. Pre-1.0, the minor is the "anything in the surface may have moved" signal.
- **PATCH** (`0.x.y`) — backward-compatible fixes only.

`1.0.0` is reserved for when the **DSL and CLI are stable enough to promise
backward compatibility** — i.e., breaking either requires a major bump. Beta is a
tagged `0.x` with the surface substantially frozen and the deprecation policy
below in force; do not cut `1.0` to mean "feature complete," cut it to mean
"stable surface."

## Deprecation policy (beta onward)

A surface element being removed or renamed must first be **deprecated**: keep it
working, emit a warning that names the replacement, and remove it no sooner than
the next MINOR (or after a stated number of releases). The warning is the
contract — silent removal is never acceptable for a beta surface.

## Conventional Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/); this
is what lets the changelog and SemVer bump be derived mechanically.

```
<type>(<scope>): <subject>
```

- **Types:** `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `ci`, `build`,
  `chore`.
- **Scopes** (the user-relevant areas the release notes group by): `dsl`,
  `dimcheck`, `cli`, `ir`, `sim`/`inference`, `obs`, `docs`, `ci`. Add scopes as
  the surface grows; keep them stable.
- **Breaking changes:** a `!` after the type/scope (`feat(cli)!: …`) **or** a
  `BREAKING CHANGE:` footer. Either drives a MINOR bump pre-1.0 (MAJOR at ≥1.0)
  and lands in the *Breaking changes* section of the release notes with a
  migration line.

## Cutting a release

1. Pick the version from the commit types since the last tag (the
   `/release-notes` skill computes the recommended bump).
2. Draft notes with the `/release-notes` skill: a deterministic changelog spine
   (git-cliff or equivalent) plus a user-facing narrative grouped by area, with
   migration steps for every breaking change. The maintainer edits.
3. Update `CHANGELOG.md`, tag `vX.Y.Z`, and publish the notes.

The version is a promise to users, not a build counter — bump it for what changed
in the surface above, and say what changed in language they can act on.
