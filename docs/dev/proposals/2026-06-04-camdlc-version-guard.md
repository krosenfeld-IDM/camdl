# Key the camdlc↔camdl guard on IR schema version, not git hash

Status: Proposed. A workaround landed (`build(make): resolve fresh camdlc
via PATH-prepend in test-rust`, 458c8cb); this proposes the root-cause fix
so the dev workaround is no longer needed.

## Problem

`camdl` (runtime) refuses to run a `camdlc` (compiler) whose **git hash**
differs from its own (`rust/crates/cli/src/util.rs`: `find_camdlc` →
`check_camdlc_version_once` → `eval_version_output`, comparing
`camdlc --camdl-version` against `crate::version::GIT_HASH`). The guard
exists to stop a runtime reading IR a mismatched compiler emitted.

Keying on exact git hash is **stricter than the actual compatibility
contract**, which is the IR schema (`ir/schema.json` / `ir/VERSION`). Two
binaries built at different commits with an *identical* schema are fully
compatible, but the guard rejects them on the commit label. So it
false-reds on essentially every dev iteration where the two sides were
built at different commits — which is the common case, because:

- a parallel checkout running `make install` clobbers the shared
  `~/.local/bin/camdlc` to a different commit; and
- cargo does not rebuild the `camdl` binary when only OCaml/docs change, so
  its embedded `GIT_HASH` lags HEAD while a freshly-built `camdlc` does not.

The reproduction that motivated this: a session of OCaml-only changes (no
schema change) produced `error: camdlc version mismatch` in the cargo
acceptance tests, against a `~/.local/bin/camdlc` left at a prior commit by
other work — a pure false-red.

## Current workaround (shipped)

`make test`'s `test-rust` prepends the freshly-built `camdlc` to PATH and
sets `CAMDL_SKIP_VERSION_CHECK=1`. The PATH-prepend pins the compiler under
test (no divergence — `camdl` never falls back to a stale PATH camdlc); the
skip mutes the commit-label mismatch against a cargo-cached stale `camdl`
binary, which is safe because **`camdl` only goes stale when no Rust
changed → the IR schema is unchanged → a stale `camdl` is schema-compatible
with a fresh `camdlc` by construction**. It works, but it is a workaround:
the guard is muted in the harness rather than passing on its merits.

## Proposed fix

Gate on **IR schema compatibility**, not git hash. The load-bearing design
choice is *what* represents the schema:

- **Not the hand-maintained `ir/VERSION` integer alone.** `ir/VERSION` is
  bumped by hand (CLAUDE.md "Changing the IR schema"). An OCaml-only change
  to the emitted IR *shape* in `ocaml/lib/ir/serde.ml` (a new field, a
  renamed tag, a reordered variant) that forgets the bump would leave
  `ir/VERSION` matching while the emitted shape diverges from what `camdl`'s
  deserializer expects — a real incompatibility a VERSION-keyed gate would
  wave through, defeating the guard's purpose. (This is also why the
  "staleness ⟹ schema-compatible" argument below is *not* airtight for the
  permanent gate — only for the same-tree harness workaround, where both
  sides share one `serde.ml` regardless of `ir/VERSION`.)
- **A content hash of the schema surface.** Derive the gate value from a
  hash of `ir/schema.json` (or the serde surface itself), embedded in both
  binaries; `camdlc --camdl-version` reports it and `camdl` compares against
  its own. An emit-shape change then moves the hash even if `ir/VERSION` is
  forgotten, so the gate trips. (Alternatively/additionally, a CI check that
  fails any commit touching `serde.ml`/`schema.json` without an `ir/VERSION`
  bump — belt and braces.)

With a schema-content gate, a matched-schema pair passes *on its own merits*
— no skip in dev or prod, no PATH-prepend needed — and a genuine schema
change still trips it.

## Tradeoff to settle

Schema version is **coarser** than git hash: it will not catch an
expander/codegen behavior change that alters emitted IR *without* bumping
the schema — e.g. an autodiff/`rate_grad` fix, or a dimensional-rescale
correction. Those change the *values* in the IR, not its shape, so a
schema-version guard would treat a fixed and an unfixed `camdlc` as
interchangeable.

Whether that matters depends on the guard's job. For the *deserialization*
risk it nominally protects against (runtime can't read the compiler's IR),
schema version is exactly right. For "am I running the camdlc I think I
am," it is not. The honest design is probably two signals:

1. **Schema version — hard gate** (refuse on mismatch). This is the real
   incompatibility.
2. **Git hash — soft warning** (print, do not exit) when it drifts, so an
   operator running a knowingly-mismatched pair is told, without blocking
   dev iteration.

This also lets the harness drop both the PATH-prepend and the skip: a
freshly-built pair shares a schema version and passes the hard gate, and
the soft hash warning is harmless noise (or suppressed under
`CAMDL_SKIP_VERSION_CHECK`).

## Recommendation

Implement the two-signal guard (hard schema gate + soft hash warning) in
`util.rs::eval_version_output`, embed a **content hash of the schema
surface** (not the `ir/VERSION` integer) in both binaries, and once it
lands, simplify `test-rust` back to a bare `cargo test --workspace`
(removing the PATH-prepend + skip workaround). Add a unit test for
`eval_version_output` covering: schema match + hash match (pass, no warn),
schema match + hash drift (pass + warn), schema mismatch (hard fail).
