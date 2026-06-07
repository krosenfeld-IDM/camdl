# Lifecycle / effect test-surface audit

Date: 2026-06-07
Purpose: pin what "exhaustive but not overkill" coverage of the within-substep
lifecycle (effects × backends) means, map the existing tests onto it, and list
the real gaps — so we solidify the test surface now that the effect-resolution
seam has landed.

The naive cross-product (4 backends × 3 effect-kinds × 4 actions × 2 arenas × ~5
edges ≈ 480 cells) is intractable and full of impossible/redundant cells. The
audit reduces it by three observations and structures coverage as five layers.

## Feasibility (grounded in `capabilities()`)

| capability | chain | tau | ode | gillespie |
| --- | --- | --- | --- | --- |
| scheduled intervention | ✓ | ✓ | ✓ | ✓ |
| always-active event | ✓ | ✓ | ✓ | ✓ |
| **balance** | ✓ | ✗ | ✗ | ✗ |
| overdispersion | ✓ | ✓ | ✗ | ✗ |
| `REAL_COMPARTMENTS` (declared) | ✓ | ✓ | ✓ | ✓ |
| **advances a real reservoir (RK4)** | ✓ (#3) | ✓ `tau_leap.rs:351` | ✓ (native) | ✓ `gillespie.rs:233,300` |

RESOLVED (review): tau_leap and gillespie BOTH RK4-integrate evolving real
compartments via the same `rk4_step` ODE uses, apply real effects, and read the
live coupled value in rates. Verified empirically (cholera_siwr: W evolves on all
four backends, peaks 735–899, ODE → deterministic 140.4). So the real cells are
**supported-but-untested** (a coverage gap, NOT a bug, NOT rejected). The chain
stale-real incident (#3) does not generalize: tau/gillespie mutate `real_s`
in place and never had the unsynced `scratch.real_s` that caused #3.

## The three reductions

1. **Capability feasibility** kills the impossible cells (balance = chain only;
   the real sub-capabilities above).
2. **The arithmetic is shared.** `effects::resolve_action` is ONE function
   (round/floor/clamp/arena dispatch). It is unit-tested once; the per-backend
   layer tests *wiring + order*, not per-backend arithmetic. So the per-backend
   grid is one action per (backend × kind × arena), not all four.
3. **Agreement collapses rows.** One non-trivial cross-backend agreement test
   covers a whole row of backends at once.

## The five coverage layers (this is what "exhaustive" means)

**L1 — shared arithmetic (unit, `effects.rs`).** `{Add, Set, FractionTransfer,
AbsoluteTransfer}` × `{int-discrete, int-continuous(ODE), real}` + apply-order +
error arms `{non-finite, negative-add, mixed-arena, clamp transfer>src}`.

**L2 — per-backend wiring.** For each backend: `{event, intervention}` × `{int,
real}` + `balance` (chain only), ≥1 action, asserting the **post-effect state
with a hand-computed number** (not a hash, not "ran"). ~17 cells.

**L3 — cross-backend agreement (non-trivial).** Same lifecycle model, all valid
backends agree on the deterministic / in-distribution outcome, with **rate ≠ 0**
so agreement is informative. Scenarios: intervention-only, event-only,
event+intervention coincident, off-grid effect.

**L4 — lifecycle ordering.** event-before-intervention; balance-last (chain);
event reads the **pre-advance snapshot** (not post-advance state); intervention
reads post-event state. Per backend or via L3.

**L5 — edges / errors / timing.** non-finite · negative · mixed-arena · clamp
(transfer > src) · off-grid effect time · coincident-with-obs/output · fractional
output end · zero-rate. Mostly covered by the Tier-1 guards + corner-case corpus.

## The non-vacuous bar (for THIS audit)

A lifecycle test earns its cell only if it:
- asserts the **post-effect state with a hand-computed value** — not a trajectory
  hash, not `is_ok()`, not "the column changed";
- for **agreement**, uses a model that **actually does something** (rate ≠ 0), so
  agreement is evidence, not a tautology;
- carries a **discriminating control** — the effect moved state by the expected
  amount *and* a sibling where it must not fire stays put;
- for **timing**, asserts the effect fired at the **right step**, not just that a
  jump occurred somewhere.

Known vacuity to fix, not just fill:
- `cross_backend_lifecycle_agreement::all_backends_agree_on_coincident_event_intervention`
  bakes `k = 0` (rate ≡ 0) — agreement is trivial; upgrade to a non-zero-rate
  model.
- `gate_corner_case_baseline` / `gate_trajectory_baseline` are **hash regression
  catchers**, not behavioral oracles. They pin "didn't change," not "is correct."
  They count as L5 regression coverage, NOT as L2/L3 behavioral cells.

## Coverage map (behavioral = hand-computed post-effect state)

L1 shared arithmetic (`sim/src/effects.rs::tests`, 11 + `statistical_distribution::
test_fraction_transfer_edge_cases`): all 4 actions × int + Add/Set on real + the
continuous Set/FractionTransfer + clamp/floor/zero-source/mixed-arena/negative-add
errors. **Strong.** Holes: AbsoluteTransfer on real (continuous), a FractionTransfer
whose *source* is a real compartment.

L2 per-backend wiring — behavioral cells that exist:

| backend | scheduled intervention | always-active event | balance |
| --- | --- | --- | --- |
| chain | FractionTransfer/int (interventions, snapshot_projections); Set/int *error-path only* | Add/real (event_on_real) | — *(hash only, `all_lifecycle`)* |
| ode | Set/real (intervention_on_real) | Add/real (event_on_real) | n/a |
| **tau** | **none behavioral** | **none behavioral** | n/a |
| **gillespie** | **none behavioral** | **none behavioral** | n/a |

L3 agreement: `events_backend_parity` (Add event, all 4 backends, but `jump≥90`
*threshold*, not exact); `cross_backend::all_backends_agree…` (Add+FractionTransfer,
coincident, all 4, hand-exact A=75/B=75 + inverted-order control) — **but rate≡0**, so
it's a genuine **L4 ordering** test, NOT an L3-under-flow agreement test.

L4 ordering: event-before-intervention (cross_backend, rate≡0); event-before-snapshot
(snapshot_projections, chain, S=500/V=500); event-reads-pre-advance-snapshot
(chain≡tau, **hash-only**; pgas_event_density Add td=0).

L5 edges/errors: non-finite + negative (value_guards); mixed-arena + negative-add +
clamp (effects.rs, statistical_distribution); off-grid+event PF/PGAS rejection
(inference_event_misfire_guard, pgas exact-reject); AbsoluteTransfer fire-count
dt-invariance/wall-time (intervention_dt_invariance — but unit-level via
`apply_interventions_at`, not a backend substep loop). The corner-case hashes
(`gate_corner_case_baseline`) are **regression catchers, not behavioral** — off-grid,
coincident, fractional-end, full-lifecycle all flow through them as hashes only.

## Gaps, ranked

1. **tau_leap and gillespie have zero behavioral effect coverage.** No hand-computed
   post-effect state for either, any action, any arena — they live only in the rate≡0
   agreement (75/75), the threshold parity test (≥90), and the hash gates. Their effect
   *wiring* is unpinned by an exact number. **Biggest hole.**
2. **No rate≠0 cross-backend agreement (L3-under-flow).** The headline cell. Upgrade
   `all_backends_agree…` (or add a sibling) to a non-zero-rate model where ODE = the
   deterministic mean and the stochastic three agree in distribution / on a conserved
   quantity.
3. **`Set` happy-path is untested across backends.** Only error-path (chain) + real
   (ode). No successful integer Set asserting the post-state, no cross-backend Set.
4. **`balance {}` has no behavioral test.** One chain hash (`all_lifecycle`); nothing
   asserts post-balance conservation (`total == N0`) or balance-last ordering.
5. **`AbsoluteTransfer` per-backend / real.** Only the unit-level fire-count driver
   (int); no backend-run state assertion, no real arena.
6. **Real-arena breadth.** Only Add-real (ode+chain) and Set-real (ode).
   FractionTransfer-real, AbsoluteTransfer-real, and *any* tau/gillespie real-arena
   effect are absent — and **whether tau/gillespie advance vs only-hold a coupled
   reservoir is unverified** (the feasibility-table `?` cells).
7. **Off-grid effect through a real backend substep loop** asserting hand-computed
   state — none (the dt-invariance tests walk `apply_interventions_at` manually).

Housekeeping surfaced: `gate_corner_case_baseline.rs:130` cites a **nonexistent**
`gate_substep_time_sdt.rs`; `gate_trajectory_baseline` `model.interventions.clear()`s
(no lifecycle); corner-case fixtures use only Add + FractionTransfer, int-only;
`events_backend_parity` asserts a threshold not the exact +100.

## Review refinements (what actually matters)

The scoping review found the structural fact that collapses the grid: **all four
backends apply *interventions* through ONE shared path** (`apply_post_advance` →
`apply_interventions_at` → `effects::resolve_*`). So per-backend intervention
wiring is one cell, not four — a strong agreement test subsumes it. The ONLY
backend-divergent effect path is the **event PROPOSE/fuse** stage: chain/tau fuse
into `pending_deltas`; gillespie applies to the snapshot via `apply_events_at`.
Consequences:
- **tau is already covered** — `chain_and_tau_byte_identical_on_fused_event_read_source`
  is a *differential oracle* (tau's fused path == chain's hand-checked number),
  not a regression hash. The coverage map above mis-classified it; it counts.
- **gillespie's event path is the real hole** (distinct snapshot apply, unpinned).
- **L4 ordering folds into L3** (the agreement fixture asserts canonical order via
  an inverted-order negative control).
- **Bar amendment:** a cross-backend *differential* equality (A's path == B's,
  where B is hand-anchored) satisfies the non-vacuous bar — it is an oracle, not a
  `gate_*_baseline` regression hash.

## Tests to write (the agreed minimal set — ~4, not the grid)

**T1 — cross-backend agreement under FLOW (rate≠0) + integer-Set + event-wiring.**
New programmatic model: a live transition `S --> I @ beta*S` (β>0, real stochastic
flow) PLUS an isolated compartment `V` touched ONLY by effects (no transition in
or out). Always-active event `add(V, 100) at [5]`; scheduled intervention
`set(V, 50) at [8]`. Run all four backends (chain dt=1, tau dt=0.5, ode dt=1,
gillespie). Assert, on EACH backend: `V == 100` for an output in [5,8); `V == 50`
for an output ≥ 8 (exact — V is effect-only, so deterministic and identical across
backends *under* stochastic S/I flow). Non-vacuity controls: assert S actually
depleted (flow happened, β>0) and ODE's S matches the deterministic decay. This
single test pins: event wiring on ALL backends (incl. gillespie's snapshot path),
intervention wiring on all backends, the integer-`Set` happy path, and
agreement-under-flow. Subsumes gaps #1(gillespie), #2, #3 and the dropped L2 grid.

**T2 — balance conservation + balance×intervention composition (chain).**
Chain model with `balance { target=R, expr = N0 - S - I }`, a live transition
(rate≠0), and a scheduled `fraction_transfer(S→R)` intervention. Assert
`S + I + R == N0` at every output (balance conserves through flow), AND that it
still holds in the substep where the intervention fires (cull-then-rebalance).
Discriminating control: a sibling run without balance where total drifts. Covers
gap #4 + the missed balance×intervention cell.

**T3 — L1 unit (effects.rs::tests): the two arithmetic holes.**
`AbsoluteTransfer` on a real source (exact f64, `.min(src)` no round) and
`FractionTransfer` whose SOURCE is a real compartment (exact `src*frac`, no floor).
Hand-computed values, no backend run. Closes the L1 holes.

**T4 — tau/gillespie evolving-real-coupling pin.**
Load `cholera_siwr` (W evolves via `dW/dt = xi*I − omega*W`, couples into the
infection rate). Run tau_leap AND gillespie (the unverified backends) + ode as the
deterministic reference. Assert: W *evolves* (W_end ≫ 0, not held at init) on tau
and gillespie, AND the W-coupled infection fired (S depleted) — with the negative
control that a broken coupling (W≡0) would leave S undepleted. Cross-checks the
RK4-real path on the two backends that had zero oracle coverage.

Skip (shared paths, would be overkill): per-backend intervention grids,
AbsoluteTransfer per-backend runs, off-grid-real-substep integration, a Set
cross-backend grid (folded into T1). Do NOT re-test tau's event path (covered).

## Housekeeping (fix while here)

- `gate_corner_case_baseline.rs:130` cites a **nonexistent** `gate_substep_time_sdt.rs`
  — remove the dead reference.
- (`gate_trajectory_baseline` clears interventions — by design, leave; corner-case
  fixtures use only Add+FractionTransfer/int — acceptable, the unit layer covers
  Set/AbsoluteTransfer.)
