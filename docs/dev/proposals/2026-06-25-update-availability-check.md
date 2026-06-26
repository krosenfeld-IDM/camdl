# Update-availability check

Status: **Proposed** — implementable as specified. No `run_id` / IR / golden
impact. Adds one dependency (`ureq`, shared with the future updater).

## Summary

An **explicit** `camdl check-update` that tells the user whether a newer camdl
release exists. It is _synchronous_ (the user ran it deliberately, so a network
round-trip with a tight timeout is acceptable and a visible failure is fine), so
it needs **no cache, no detached child, no per-command dispatch hook, and no
on-by-default network call** — which is what makes it small and safe.

This is the lowest-risk slice of the "camdl moves fast → users on stale binaries
silently get wrong answers" problem (e.g. the forcing-ISO bug a stale user would
hit). A _passive_ per-command nudge is a deliberately deferred follow-up — every
hard problem in the earlier auto-check design (detached-process survival, a hot-
path cache, recursion, an on-by-default phone-home from a possibly-sensitive
ministry network) is a property of the _automatic_ check, not of _checking_; the
explicit command has none of them.

## Why explicit, not a per-command auto-check

An adversarial review found that the automatic, detached, per-command design
dragged in: an unprecedented detached-process survival requirement (a naive
`Command::spawn` does not detach on Unix; `main()` exits via `process::exit`), a
recursion guard, a single-flight lock, clock-skew/offline-backoff handling, an
stderr-contract collision risk, and a default-on outbound call — for, in an
alpha with a small engaged user base who already run `git pull && make install`,
the marginal value of _passive_ nudges over a deliberate check. The explicit
command delivers the safety value (a stale user who runs it learns they're
behind) at a fraction of the surface. So: ship the explicit check; defer the
passive nudge.

## The command

```
camdl check-update     # synchronous: fetch latest release → compare → print
```

A single top-level verb, like `camdl simulate` or `camdl fit` — not a subcommand
group. It is deliberately **not** named `camdl update`: a bare `update` reads as
"perform an update," and that name should stay reserved for a future binary
updater (download + verify + replace), which is a different and riskier
operation. `check-update` says exactly what it does — checks, never mutates the
binary — so the two coexist later (`camdl check-update` to look, `camdl update`
to act) with no rename. Flow:

1. `ureq` GET the releases list with a **tight timeout** (connect+read ≈ 3 s).
2. Parse out the newest tag (see "What it compares against").
3. Semver-compare to this binary's version; print one of:
   - `camdl 0.3.0 is available (you have 0.2.0). https://github.com/vsbuffalo/camdl/releases`
   - `camdl 0.2.0 is up to date.`
   - `couldn't reach GitHub to check for updates (offline?).` — exit 0, this is
     not an error; the user asked and we simply couldn't answer.
   - `no camdl releases published yet.` — when the repo has no releases.

Because the user invoked it, a visible "couldn't reach GitHub" line is correct
(not the silent-fail an automatic check requires).

## The HTTP client: `ureq` (rustls)

camdl has **no HTTP client today** (the `axum`/`tokio` in `cli/Cargo.toml` are
unused server deps). This adds one: `ureq` with the rustls TLS backend.

- **Blocking**, so it fits the synchronous command with no async runtime
  (reqwest's blocking mode still pulls tokio; ureq doesn't).
- **rustls** = no system OpenSSL to locate at build or runtime — serves camdl's
  no-admin / cross-platform / self-contained-binary goals. (The `curl` crate /
  `isahc` link the C libcurl, worse for cross-compile; a `curl` _shell-out_ adds
  a runtime-binary dependency and stringly error handling.)
- Footprint note: the bulk of the weight is the **TLS stack (rustls)**, which
  any `https://` client needs — it is not ureq-specific. `minreq` is a smaller
  wrapper but pulls the same rustls for HTTPS, so it's a wash; `ureq` has more
  community mass and a cleaner API.
- **Shared with the future updater**, which must download _and verify_ signed
  release binaries — so this dependency is a forward investment, not notify-only
  weight.

## What it compares against

GitHub's `/releases/latest` **excludes pre-releases**, and camdl is alpha-tagged
(`v0.1.0-alpha`), so `/releases/latest` returns 404 until a stable release
exists — it would nudge nobody through all of alpha, the opposite of the goal.
So the check queries **`GET /repos/<o>/<r>/releases`** (or `/tags`) — which
_includes_ pre-releases — and takes the newest by semver. The empty case (no
releases) is a **first-class outcome** (`no releases published yet`), distinct
from a network failure.

Comparison rules (no implementer guess):

- Parse both sides with a semver crate; **strip a leading `v`** (tags are
  `vMAJOR.MINOR.PATCH`, `CARGO_PKG_VERSION` is not).
- **Pre-release ordering** per SemVer §11 (`0.2.0-rc.1 < 0.2.0`).
- Nudge **iff `latest > binary`**; equal or lower → "up to date" (a dev build
  ahead of the newest release reads as up to date — correct).
- The binary's version is `CARGO_PKG_VERSION` (bumped at release time by
  `scripts/release.sh`); the git-hash is shown for context, not used in the
  compare.

## Cache, home directory, seams

- **No cache.** The explicit command fetches every time it's run; there is
  nothing to throttle and no hot path to protect. (A cache only matters for the
  deferred passive nudge — it belongs with that follow-up, under the
  **existing** `~/.cache/camdl/` that `util.rs:663` `ir_cache_dir` already
  establishes, _not_ a new `~/.camdl/`. The `~/.camdl` home question defers to
  the updater RFC.)
- **Reuse existing seams**, don't reinvent: `std::io::IsTerminal` for any
  colorization (precedent `style.rs:46`, `main.rs:501`) and `NO_COLOR`. No
  per-command hook means no TTY/CI/`--no-progress` gating is needed for v1 (the
  command is deliberate and prints to stdout/stderr as the user expects).

## Privacy

Explicit-only means **no default-on phone-home**: camdl contacts GitHub only
when the user runs `camdl check-update`. That removes the privacy concern that
made an automatic check questionable for a health-ministry user on a sensitive
network. (The deferred passive nudge would reopen the on-by-default question —
to be decided _with_ that follow-up, leaning opt-in or first-run consent.)

## Sequencing

1. Build `camdl check-update` (ureq, `/releases`-incl-prereleases, semver
   compare).
2. **Mint a release** so the check has something to report. Because the check
   uses `/releases` (not `/releases/latest`), an alpha/pre-release tag (e.g.
   `v0.2.0` or `v0.2.0-alpha`) works as the baseline — no need to wait for a
   "stable" tag.

## Deferred follow-ups (named, not built here)

1. **Passive per-command nudge** — the cache-and-detach design, _only if_ the
   explicit check proves insufficient at keeping users current. It carries the
   real costs the review flagged (detached-process survival + a test proving it,
   a `~/.cache/camdl/` cache with single-flight + offline backoff, the
   on-by-default privacy decision); not worth them for v1.
2. **A documented `.bashrc` / `.zshrc` snippet** — a shell one-liner users opt
   into that reminds them on shell start. Opt-in by construction (zero in-binary
   surface, no privacy default), so it's the lightweight passive option; ship it
   as docs, not code, when wanted.
3. **The binary updater** (`camdl update` — download + verify signed binaries +
   replace) and the `~/.camdl/versions/` home — a separate RFC; it reuses this
   proposal's `ureq`. Naming the checker `check-update` leaves `update` free for
   this verb.

## Decisions recorded

- **Explicit `camdl check-update`** (a single top-level verb, not a flag and not
  a subcommand group), synchronous; no cache / no detach / no per-command hook /
  no default-on network call. `update` stays reserved for the future binary
  updater.
- **`ureq` + rustls** as the client (shared with the future updater); not a
  shell-out, not reqwest, not a libcurl binding.
- Query **`/releases`** (includes pre-releases) so an alpha-tagged project
  nudges; the no-release case is a first-class outcome.
- Semver compare with `v`-strip + pre-release ordering; equal/lower → up to
  date.
- Cache + `~/.camdl` home are deferred to the passive nudge / updater, which use
  the existing `~/.cache/camdl/` and a separate RFC respectively.
