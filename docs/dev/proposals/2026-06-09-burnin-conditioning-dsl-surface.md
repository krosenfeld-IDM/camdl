# Burn-in / conditioning window: the DSL surface and modeler options

- **Status:** Draft **v2** — revised 2026-06-09 after a design review that found
  a design-breaking error in v1 (the default rule below). See §0.
- **Relationship:** the _DSL-surface + modeler-UX companion_ to the pinned
  inference-math proposal
  [`2026-05-30-conditioning-boundary-tcond.md`](2026-05-30-conditioning-boundary-tcond.md)
  (owns the unweighting mechanism, PGAS/IF2 threading, and the **open §3
  warm-up-semantics crux**). Blocked on that §3 crux.
- **Issues:** gh#134. Its substantive asks have **already shipped** (see §7);
  the remaining work is the unweighting fix (pinned proposal) + visibility.
- **Required reading before implementing:** `docs/camdl-language-spec.md` §2.1
  (units/time) + §7 (forcings); `docs/dates.md`; `parser.mly` `simulate_kv`; the
  pinned proposal end-to-end (esp. §3).

## 0. Corrected thesis (what the design review surfaced)

**v1 of this doc was wrong** in a load-bearing way: it proposed a
`condition_from` keyword as the burn-in surface and a default that "conditions
from `first_obs`, as today." But "as today" is the gh#134 **bug** — that leading
window is _scored_ (loglik −3416). The covariate-informed burn-in this doc is
named for **needs no new DSL keyword**. It is already expressible as:

> **`simulate.from` set early** (exists today) **+ the leading window
> `[t_start, first_obs)` scored _unweighted_** instead of scored-as-today.

That unweighting **is** the pinned proposal's default (`t_cond = first_obs`).
So:

- **Bug (gh#134):** `[t_start, first_obs)` is _scored_ → −3416.
- **Fix (pinned proposal):** that window becomes an _unweighted warm-up_ → the
  covariates shape `S` over it, conditioning starts at the first datum.
- A `condition_from` keyword does **not** drive this. The only thing it could
  add is pushing conditioning _past_ `first_obs` (deliberately discarding
  leading observations) — a **separate, rarer** feature (§3).

So the answer to "support a `burnin` keyword in the `sim` block?" is **no
keyword for the burn-in itself.** The clean surface is three things, none of
them a new grammar keyword: (a) the unweighting fix (pinned proposal), (b) a
fit/dry-run **header echo** so the warm-up is _visible_, (c) the
**already-shipped W329 guard** for the _accidental_ case.

## 1. Problem (brief — full reproduction in the pinned proposal)

The filter conditions observation _k_ over `[t_{k-1}, t_k]`; the first window is
`[t_start, first_obs]`. When `simulate.from` (→ `t_start`,
`expander.ml:~3576/3590`) sits far behind the first datum, that window spans the
whole gap — the model **free-runs unconditioned** and the first incidence window
accumulates a giant flow. Reproduced (gh#134, Kano measles, only `simulate.from`
moved):

| `simulate.from`       | first window       | loglik |
| --------------------- | ------------------ | ------ |
| origin (2011)         | `[0, 980]` = 980 d | −3416  |
| 1 wk before first obs | `[973, 980]` = 7 d | −3202  |

The **legitimate** use is a covariate-informed burn-in: births accumulate
susceptibles and SIA/MCV deplete them over 2011–2014, so the covariates are
informative about `S(2014)`. The user wanted this, couldn't express it, and fell
back to estimating a free `initial_susceptible_fraction` (an IVP) — "a real loss
of rigor" (§5).

## 2. Why the burn-in needs no new keyword — just unweighting + visibility

`simulate.from` _already_ decouples where dynamics begin from where the data is.
The gap is **not** "the modeler has no keyword"; it is two specific things:

1. **Inference:** `[t_start, first_obs)` is _scored_ (the bug) and must be
   _unweighted_ (the pinned proposal's mechanism). This is inference math, not a
   surface — it belongs to the pinned doc.
2. **Visibility:** nothing tells the modeler a warm-up is happening. Fix with a
   **fit/dry-run header echo** (the "design for humans" lever) — e.g.
   `warm-up [2011-12-26, 2014-08-25) = 2.66 yr, unweighted; conditioning from
   2014-08-25 (first obs)`
   when `origin` is set (cf. `simulate --dates`). No new grammar; the warm-up
   becomes legible on every run.

Plus the **shipped W329 guard** already warns when the leading window ≫ the
modal cadence and `condition_from` was _not_ set deliberately — catching the
_accidental_ version of exactly this.

## 3. The one optional knob: `condition_from` (discard data past `first_obs`)

The single thing `simulate.from` + unweighting cannot express is starting
conditioning _after_ `first_obs` — deliberately discarding the first K
observations (e.g. "the first season's reporting is unreliable"). If modelers
want that:

- A `sim`-block `condition_from = date("…")` is grammar-clean: it lexes as
  `IDENT` (not a reserved token), so `simulate_kv` routes it exactly as it
  already routes `dt` (`parser.mly:~703-714`) — no ambiguity. It takes
  `date(...)`/offset, mirroring `from`/`to`.
- Domain: `condition_from ∈ [first_obs, to)`. `< first_obs` is meaningless (no
  data to condition on); `= first_obs` is the default; `> first_obs` discards.

**Recommendation: defer `condition_from` unless a modeler asks.** It does not
drive the headline burn-in, and adding a knob that _looks like_ it controls the
warm-up but doesn't is a footgun. The burn-in ships via §2; this is a separable
follow-up.

## 4. Modeler options menu

1. **Interventions/events in the warm-up `[t_start, first_obs)` MUST fire.** The
   Kano SIA campaigns _are_ interventions/events in 2011–2014 — depleting
   susceptibles is the whole point. The warm-up is unweighted, not un-simulated;
   scheduled `at [...]` interventions and every-substep events fire normally.
   (This is the load-bearing item v1 missed.)
2. **Covariates/forcings apply in the warm-up.** Forcing tables are indexed on
   absolute time `t`, so no re-anchoring to `t_start`/`condition_from` is needed
   — state this so a reader doesn't ask.
3. **Header echo (calendar terms)** — §2.
4. **Per-stream conditioning** — heterogeneous streams may begin at different
   times; a single global boundary is too coarse. _Defer to the obs-data
   surface._
5. **`to` / `t_end` symmetry** — this doc decouples the _start_; the symmetric
   end question (can conditioning stop before `to`, for a forecast-only tail?)
   is **out of scope** here. Named so it isn't forgotten.
6. **No `burnin = <duration>` sugar.** Besides the anchor-ambiguity of a bare
   duration, in anchored mode `burnin = 3 'years` is itself an **`E321`**
   (calendar-duration offset from an `Instant`, spec §2.1) unless lowered via
   `add_calendar_*` — extra reason the boundary, not a duration, is the right
   primitive _if_ §3 is ever built.
7. **Warm-up semantics (the §3 crux)** — owned by the pinned proposal; this doc
   does not resolve it.

## 5. Interaction with IVP (the deepest link — and it favors simplicity)

Today, "I don't know the initial state" is expressed with **IVP parameters**
(`ivp`; PGAS auto-detects them as params that perturb `initial_state`, and draws
`Binomial(N, frac)` per particle **at `t_start`** — verified `pgas.rs:~1002`
draw, `~1663` detection; `initial_state(params)` takes no time arg — it is the
seed at the start of dynamics). Burn-in is a **second way to handle the same
uncertainty**, often more principled:

- **IVP** = _estimate_ the initial state at `t_start` as free parameters.
- **Burn-in** = _seed_ a simple/known state at `t_start` and let **dynamics +
  covariates** carry it to `first_obs`, so the boundary state is _derived_, not
  estimated.

The pinned proposal's motivating failure is exactly this trade — the user
abandoned the burn-in and estimated a free `initial_susceptible_fraction`. So
**a working burn-in reduces the IVP burden.** Design points:

- **IVP draws seed at `t_start`** (start of dynamics), not at the conditioning
  boundary — seeding at the boundary would make the warm-up pointless.
- **The IVP spread is created per-particle at `t_start` regardless of warm-up
  style.** What differs downstream: a _stochastic_ warm-up **preserves and
  transforms** that spread to `first_obs`; a _deterministic skeleton_ warm-up
  **collapses** it (all particles share one trajectory) — which is precisely the
  pinned §3 "zero process variance / degenerate ESS at the boundary" failure. So
  the two features compose correctly only under a stochastic warm-up — another
  reason §3 must be settled first.
- **Guidance (not a hard rule):** prefer a covariate-informed burn-in over a
  free initial-state IVP for the _susceptible pool_ (covariates carry real
  information); keep IVP for genuinely unobserved initial compartments
  (`e0`/`i0`) where no covariate informs them. Complementary, not exclusive.

## 6. The guard: shipped soft-warn vs. the pinned hard-error decision

A **doc-vs-code divergence** to reconcile: the shipped first-interval guard
(**W329**, `util.rs:970`, soft `[warn …]` that never rejects) diverges from the
pinned proposal §4, which **decided** the guard should be a **hard error +
opt-out**. The hard-error rationale was "there's no way to express the
intentional case" — but there _is_ (an early `simulate.from` _is_ the
intentional warm-up, once §2's unweighting lands). So the right end state is
plausibly: a **soft W329 warn** for the accidental case + the unweighting making
the intentional case _correct_, rather than a hard error. **Decide explicitly**
when §2 lands; until then W329-as-warn is the conservative interim.

## 7. Status & sequencing

- **Done (already shipped):** gh#134 Request 1 (model-side nudge,
  **W324/W325**); gh#134 Request 2 (first-interval warning, **W329** — its code
  comment mislabels it "request 3"). gh#134's substantive asks are complete.
- **Blocked** on the pinned proposal's §3 (warm-up semantics) + the obs-data
  surface: the **unweighting fix** (the actual burn-in, pinned-doc inference
  math) + the **header echo** (§2).
- **Optional / deferred:** `condition_from` discard-past-`first_obs` (§3);
  gh#134 Request 3 (the dry-run/fit-header date echo — cosmetic, can fold into
  §2's echo).
- **gh#134 disposition:** closeable now (substantive asks shipped); the real
  remaining work lives in the pinned proposal, and the cosmetic dry-run echo can
  be a tiny follow-up or fold into §2.

## Open questions for the maintainer

1. Do modelers actually want `condition_from` (discard past `first_obs`), or is
   the burn-in (early `simulate.from` + unweighting + header echo) sufficient?
2. **W329 soft-warn vs the pinned §4 hard-error** — final call once §2 lands.
3. Is a header echo enough to make the warm-up legible, or is an explicit
   in-model marker wanted despite the "no new keyword" conclusion?
