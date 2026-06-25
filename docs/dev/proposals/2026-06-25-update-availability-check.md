# Update-availability check

Status: **Proposed** — implementable as specified. Establishes `CAMDL_HOME`
(`~/.camdl`) as camdl's user-side home. No `run_id` / IR / golden impact.

## Summary

A non-blocking, fail-silent check that tells a user when a newer camdl
**release** is available, run on every subcommand via the central dispatch. The
defining constraint: **the network is never on the command's critical path.** A
`camdl simulate` must never wait on github.com, never hang on a dead network,
and never corrupt its output. The check therefore adds sub-millisecond latency,
works offline, and degrades to a silent no-op when there's no network.

It also establishes `~/.camdl/` (overridable via `CAMDL_HOME`) as camdl's home —
the same home the future versioned installer/updater will use.

## The non-negotiable: cache-and-detach

Every invocation consults a **local cache**; only a throttled, **detached**
child ever touches the network.

1. On dispatch, read `$CAMDL_HOME/cache/update-check.json`.
2. **Cache fresh** (checked < `TTL` ago, default 24h): decide the nudge from the
   cached value. **Zero network.**
3. **Cache stale**: do _not_ fetch inline. Spawn a fully **detached** child
   (`camdl __update-refresh`, a hidden subcommand) that does the fetch and
   rewrites the cache, then the foreground command returns immediately. The
   _next_ invocation reads the fresh cache.

The network round-trip thus happens in a process that outlives the command. The
command never blocks on it.

## The app home: `~/.camdl/` (`CAMDL_HOME`)

```
$CAMDL_HOME/                    # default ~/.camdl, overridable via CAMDL_HOME
├── cache/
│   └── update-check.json       # this feature — disposable, regenerable
├── versions/                   # FUTURE (updater): managed binary pairs
└── config.toml                 # FUTURE: user config
```

One namespace (precedent: `~/.cargo`, `~/.rustup`, `~/.npm`),
`CAMDL_HOME`-overridable (precedent: `CARGO_HOME`/`RUSTUP_HOME`) so shared
systems and tests can redirect, and **subdivided by purpose** so the disposable
`cache/` never sits next to a precious `versions/` install (clearing the cache
can't nuke an install). Strict XDG is deliberately not pursued — it scatters
camdl across `~/.cache`, `~/.local/share`, `~/.config`; the single
`CAMDL_HOME`-overridable home is simpler and covers the shared-system case.
(Optional nicety: if `$XDG_CACHE_HOME` is set _and_ `$CAMDL_HOME` is not, the
cache may live there — not required.)

This feature creates only `cache/`; `versions/`/`config.toml` are named so the
home is designed for them, not built here.

## Types and flow

```rust
/// Cached result of the last update probe. Atomic-written by the detached
/// refresher; read by the dispatch hook. Schema-versioned so a future field
/// can't mis-parse an old file (treat a parse failure as "no cache").
struct UpdateCache {
    schema:          u32,
    checked_at:      i64,            // unix seconds; the TTL gate
    latest_version:  Option<String>, // latest release tag, e.g. "0.3.0"; None = unknown
    latest_url:      Option<String>, // release html_url, for the nudge line
    last_attempt_ok: bool,           // false after a failed fetch — don't retry-storm
    last_notified:   Option<String>, // version we already nudged about; nudge once per version
}

/// Called once at the top of dispatch, for every subcommand. Pure aside from a
/// cache read, an optional detached spawn, and an optional stderr line. Never
/// returns an error, never blocks, never touches stdout or `run_id`.
fn maybe_notify_update(stderr_is_tty: bool);
```

`maybe_notify_update`:

- returns immediately if `CAMDL_NO_UPDATE_CHECK=1`, or `CI` is set, or
  `!stderr_is_tty`;
- reads the cache (a parse failure → treat as empty, no panic);
- if fresh and `latest_version` is newer than this binary and
  `!= last_notified`: print **one line to stderr** and record `last_notified`;
- if stale: spawn the detached refresher (under a lock so concurrent commands
  don't storm it) and return — never wait on it.

The refresher (`camdl __update-refresh`) is fully detached (Unix: `setsid` /
double-fork so it survives the parent), fetches with a tight timeout (≈3s), and
**atomic-writes** the cache (temp + rename). On any failure (no network, DNS
timeout, GitHub down, non-2xx) it writes `last_attempt_ok=false` and exits 0 —
silently. It holds a lockfile (`cache/refresh.lock`, atomic create) so only one
refresher runs at a time; a stale lock (> a few minutes) is reclaimable.

## Latency and no-network guarantees

- **Cache hit** (the ~always case): one small file read + a semver compare →
  **sub-millisecond**, effectively free.
- **Cache miss** (≤ once per TTL): read + spawn a detached child + return →
  **~1–3 ms** foreground; the DNS/TLS/HTTP (50 ms–seconds) is entirely in the
  background child.
- **No network:** cache hit unaffected; cache miss → the detached child fails
  silently and the foreground already returned. **Zero impact, no hang, no
  error.**

Contrast: a synchronous probe per command would be 100 ms–to–multi-second-hang
on slow DNS / no network. The detach is mandatory, not an optimization.

## Guards (so "every subcommand" is safe, not annoying)

- **stderr only, TTY-gated** — never corrupts piped / `--json` output; auto-skip
  when stderr isn't a TTY or `CI` is set.
- **Opt-out** `CAMDL_NO_UPDATE_CHECK=1`.
- **Nudge once per version** (`last_notified`) — the cheap cache read runs every
  command, but the line prints at most once per newer release, not every call.
- **Throttled** by `checked_at` + `TTL` (24h default); **atomic** cache write;
  **single-flight** lock on the refresher.
- **`run_id`-neutral and output-neutral** — a stderr cosmetic; never affects the
  command's result, so reproducibility and tests are untouched.

## The seam

A single call from the central dispatch (`main.rs`, before the matched
subcommand runs), so coverage is by construction, not copy-paste. camdl already
carries its own version (`version.rs`:
`CARGO_PKG_VERSION + "+" + CAMDL_GIT_HASH`) and a version-awareness path (the
camdlc↔camdl guard, `util.rs`); this extends that awareness outward (latest
_release_) rather than adding a parallel version system.

## What it compares against

The **latest release tag**, not raw `main`. The refresher fetches
`…/releases/latest` (unauthenticated; the 24h throttle keeps it far under the
60/hr/IP rate limit) and stores the tag + `html_url`. The dispatch hook compares
this binary's `CARGO_PKG_VERSION` (semver) to the latest release tag; newer →
nudge. A dev build between releases reports the last release's version and so
reads as "current" — correct, since the feature answers "is there a newer
**release**," not "are you behind `main`" (a dev who wants the latter checks
git). The git-hash is shown in the nudge for context, not used for the
comparison.

The nudge line (illustrative):

```
camdl: 0.3.0 is available (you have 0.2.0). Update: https://github.com/vsbuffalo/camdl/releases/latest
```

## Sequencing

This feature needs a release _tag_ to compare against — **not** binary assets.

1. Build the checker (release-aware, as above).
2. **Mint `v0.2.0`** (notes-only is sufficient) — the baseline the checker
   reports against; from then on every release nudges anyone on an older build.
3. _(Phase 2, separate proposals)_ a tag-triggered **binary release pipeline**
   (per-platform `camdl`+`camdlc` + checksums + signature) and
   **`camdl update`** (the versioned installer under `$CAMDL_HOME/versions/`).
   The nudge points at `camdl update` once it exists; until then it points at
   the releases page / `git pull && make install`.

## Out of scope (separate)

The actual updater (`camdl update`), the binary release pipeline, and a
binary-download `curl|bash` fast path. This proposal is _notify only_ — the
lowest-risk, highest-value slice (it's what turns "camdl moves fast → stale
users get silent-wrong answers" into a visible nudge).

## Privacy and security

The detached fetch reveals the user's IP and check timing to GitHub. That is the
only outbound contact; it is opt-out (`CAMDL_NO_UPDATE_CHECK=1`), CI-suppressed,
and never sends usage data. Because notify only _reads_ a version string and
never downloads or executes anything, the supply-chain surface of `camdl update`
(signature verification, etc.) does **not** apply here — that risk lands in the
Phase-2 updater proposal.

## Decisions recorded

- `~/.camdl/` (`CAMDL_HOME`-overridable), subdivided; not strict XDG.
- Cache-and-detach: the network is never synchronous; fail-silent offline.
- Wired into every subcommand via one dispatch hook; stderr + TTY-gated;
  opt-out; nudge once per version; 24h throttle.
- Compares to the latest **release** (semver), not `main`.
- Notify only; the updater + binary pipeline are separate, and the checker ships
  with `v0.2.0` as its baseline.
