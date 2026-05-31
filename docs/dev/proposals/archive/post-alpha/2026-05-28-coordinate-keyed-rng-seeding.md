# Proposal: canonical coordinate-keyed RNG seeding

Date: 2026-05-28
Status: Draft — narrow, scoped
Area: CLI orchestration / RNG seed derivation (no backend or IR change)

## The problem (what initiated this)

camdl's run determinism is **order-coupled**: correctness depends on the
RNG being seeded and consumed identically across runs (`CLAUDE.md`
§"RNG and paired-seed coupling": "any structural change that reorders
draws also breaks the coupling"). This has two layers:

1. **Cell seeding** — how each `(scenario, draw, replicate)` cell's
   process/obs seed is derived from the base seed.
2. **Within-run draw order** — the sequence of draws a backend consumes
   once seeded.

The simulate/batch unification touches **layer 1**, and layer 1 is
currently fragile for a concrete, verifiable reason: **the seed
derivation is scattered and duplicated, not canonical.**

- `main.rs:412-414` defines `SEED_MIX_DRAW`, `SEED_MIX_REP`.
- `main.rs:833-841` computes
  `process_seed = seed ^ draw_idx·SEED_MIX_DRAW ^ rep·SEED_MIX_REP`
  and `obs_seed = process_seed ^ SEED_MIX_OBS` inline in the
  `simulate` loop.
- `util.rs:17` defines `SEED_MIX_OBS` ("canonical home").
- `survey.rs:1178-1179` **re-declares `SEED_MIX_REP` locally with the
  identical value `0x517c…`** plus a `SEED_MIX_POINT`, and re-implements
  the mix at `survey.rs:1180-1181`.
- `batch.rs` derives its own per-run seeds separately.

So the rule "seed N means trajectory T" lives as inline arithmetic
duplicated across `main.rs`, `survey.rs`, and `batch.rs`. A refactor
(the `run_job` unification being the live example) that re-implements
this derivation even slightly differently silently changes scientific
output, with no error. We are currently defending this only with an
after-the-fact tripwire (`determinism_pin.rs`: seed coherence + CRN
coupling) — which *catches* breakage but does not make refactors safe by
construction.

## Who benefits

- **The simulate/batch unification** — `run_job` calls one canonical
  derivation instead of re-deriving the mix, so the reroute is
  order-independent by construction at the cell layer.
- **`batch run`** — stops maintaining a parallel seed path; "seed N"
  provably means the same thing as in `simulate`.
- **`survey` / future sweep code** — no more locally-copied
  `SEED_MIX_REP`.
- Any future engine refactor that re-orders or parallelizes the
  `scenario × draw × replicate` grid.

## The narrow solution

A single canonical, pure, tested function — the only place cell seeds are
derived:

```rust
/// The logical coordinate of one run cell. Seeds are a pure function of
/// this — independent of iteration order, parallelism, or which entry
/// point (simulate / batch / survey) produced it.
struct RngCoord {
    base_seed:  u64,
    draw_idx:   u64,   // ParamSource draw / sweep point
    replicate:  u64,
    // scenario is DELIBERATELY absent — see "CRN" below.
}

fn process_seed(c: RngCoord) -> u64;   // canonical mix, the current arithmetic
fn obs_seed(c: RngCoord) -> u64;       // process_seed(c) ^ SEED_MIX_OBS
```

- **Same arithmetic as today** (`seed ^ draw·MIX_DRAW ^ rep·MIX_REP`,
  `^ MIX_OBS`). This is a *centralization*, not a value change:
  **all golden trajectories stay byte-identical.** That is the proposal's
  acceptance gate (below) and its safety guarantee.
- The `SEED_MIX_*` constants live in exactly one module; `survey.rs`'s
  local copy is deleted and it calls the canonical function.
- Explicit-seed lists (`--seeds`) bypass the mix (use the value directly),
  exactly as today.
- **Scenario stays out of the coordinate**, preserving paired-seed CRN
  (scenarios at the same `(draw, rep)` share a seed) — the property
  `determinism_pin.rs::crn_coupling_*` locks.

This is "event-keying," but only at the **cell** granularity, reusing the
seed-derivation we already trust. It is the outer layer of the larger
event-key idea, and composes cleanly with it should we ever do the inner
layer (below).

## Acceptance

- `determinism_pin.rs` (seed coherence, CRN coupling, determinism) stays
  green.
- **`ir/golden` and all expected outputs are byte-identical** — `git
  status -- ir/` shows nothing. If any golden moves, the centralization
  changed the arithmetic; stop. (No `update-golden`/`update-expected`.)
- `survey` outputs unchanged.

## Out of scope (the larger problem, parked)

The deeper **within-run draw-order** fragility (layer 2) — making a
single firing's value independent of how many draws preceded it — is the
*per-firing event-key* system. That is a separate, larger initiative:

- It changes random *values* → regenerates every golden → requires
  statistical re-validation against pomp + the oracle/gradient suites.
- Its scientific payoff is variance-reduced **counterfactual** contrasts
  (the "placebo invariant": a null intervention yields byte-identical
  output), not refactor safety.
- **Prior art:** a fully-specified `EkRng` (event-keyed, counter-based,
  Philox/Threefry, placebo test, an `event_key` field on `Transition`)
  existed and was *abandoned at integration* — designed and partially
  coded (`make_rng = hash(seed, event_key, counter) → ChaCha8`, ahash
  seed derivation) but never wired into a backend, then deleted
  (`940da0a`, `1c12faf`, `eebcfe5`). It was never load-bearing.

This proposal does **not** revive that. It does not touch backends,
distributions, Gillespie, the cipher, or the IR. If counterfactual
variance reduction ever becomes a felt pain, the per-firing system gets
its own proposal (and the canonical `process_seed`/`obs_seed` here is the
seam it would sit behind). Until then: parked.
