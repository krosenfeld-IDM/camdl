# Systemic root causes behind the issue cluster (2026-06-08)

Synthesis of a 5-lens read-only smell-hunt (22 findings) over the critical-issue
dossier + the unified-time-system review. The goal (maintainer's words): _"I
don't just wanna tackle these issues — I want any opportunity for broader fixes
or missing gaps in testing."_ Each root cause below, fixed once, retires a
family of open issues. Agent-produced; spot-check file:line before acting.

Collision key: 🔴 = primary edits land in the other agent's files
(`sim/src/inference/*`, `effects.rs`, `lifecycle.rs`) → coordinate. 🟢 = clear.

---

## RC1 — Value and gradient densities are three hand-maintained copies that drift 🔴

**Retires:** unified-time #1 (gamma value missing from grad path), #5 (determ
inflow scored Poisson in grad only), #20 (ungrouped `rate<=0.0` vs
`RATE_EPSILON`), #79, #119-partB, and the σ²-zeroed-state note (gh#195).

The per-substep stochastic density exists three times: the forward draw
(`chain_binomial::step_one`), the PGAS **value** (`complete_data_loglik` /
`log_transition_density_substep`), and the PGAS **gradient**
(`complete_data_loglik_grad` / `log_transition_density_grad`). The gradient copy
has silently dropped terms the value copy has (gamma value, `is_determ`
exact-count branch, the `RATE_EPSILON` guard). The grouped path even carries a
"Now mirrors pgas.rs exactly" comment while the ungrouped path 100 lines below
does not.

**Broader fix:** make value and gradient ONE traversal returning
`(value, grad)`, so a term cannot be present in one and absent in the other (the
gh#197 fix already names this: have `log_gamma_density_grad_substep` return
`(value, grad)`). **Meta-test:** a value-of-grad-fn vs value-fn consistency
oracle — `complete_data_loglik_grad(θ).0 == complete_data_loglik(θ)` for the
same trajectory (today's FD tests only check `grad ≈ FD(value-fn)`, so a term
missing from the grad-fn's _value_ is outside the assertion surface).

## RC2 — Capability gating is forked, opt-in-per-callsite, multi-source-of-truth 🟢

**Retires:** gh#192, gh#191, gh#95(declaration), gh#119(flag), review #15 (Dt).

"What does this backend support" is asserted in 4+ disconnected places that have
drifted from each other and from runtime reality: `Simulate::capabilities()`
(trait), the hardcoded `match backend` in `check_model_capabilities`, the
`util.rs` forward gate, and `required_capabilities()` (which scans only 3
feature classes and never walks rate ASTs). Some commands gate via the drifted
fork (profile — too strict), some not at all (fit run, `pfilter`,
`survey --eval
pfilter` — too lax).

**Broader fix:** this is the **capability-gate-consolidation proposal**
(`docs/dev/proposals/2026-06-08-capability-gate-consolidation.md`, v2). The
smell-hunt independently confirmed the direction _and_ extended it: make
capabilities a function of **(backend × ExecMode{Forward,Inference})** that is
_derived_ not hand-listed; make `required_capabilities()` the single exhaustive
IR scan and add the missing flags (`PARAMETRIC_FORCING`, `RUNTIME_DT`,
`REAL_COUPLED_RATE`). **Meta-test:** enumerate {backends}×{modes}×{features} and
assert each (supporting backend sensitive / non-supporting refuses), routed
through one Result-returning gate every command calls.

## RC3 — Effect-action application is duplicated across int/ODE arenas + ad-hoc count casts 🔴

**Retires:** unified-time #4 (neg add (-0.5,0) bypass), #6 (coincident Set
diverges chain vs ODE), #14 (ODE hardcoded `t:0.0`), dossier #124 (silent
neg/frac/NaN init casts), premerge `set(real,<0)`.

`resolve_action` (discrete) and `apply_action_f64` (ODE) are parallel
hand-copies of the same 5 Action semantics; guard logic, error metadata, and Set
semantics have all drifted. Both guard the **rounded** value (so a (-0.5,0)
amount slips through). Float→int count conversion is per-callsite
(`round`/`as i64`/`floor`) with the guard testing the already-rounded value.

**Broader fix:** one `resolve_action_amount(action, v, read_state)`
parameterized over arena (not copied per arena), and one
`checked_count(name, v, t) -> Result` helper (reject `!is_finite`, reject raw
`v<0.0`, reject non-integer, then cast) routed at every count-producing site.
**Meta-test:** cross-backend agreement (chain/ode/gillespie identical
post-effect counts) + a property test over `checked_count` for
`{NaN, ±inf, -3, -0.3, 0.6, 1e20, 2.5}`.

## RC4 — No single complete reference-resolution + validation pass 🟢

**Retires:** gh#111 (string-concat indexed refs), #112 (table arity), #114
(stratified init), #117 (duplicate/cross-namespace names), #123 (Rust validator
skips most refs), #124.

Validation is split across the OCaml expander (ad-hoc per-construct E-codes) and
two IR validators (`validate.ml` / `validate.rs`) that walk only 3 ref-bearing
fields — neither owns completeness, and names are built by `String.concat` with
no membership/arity reconciliation. So init keys, table arity, balance/add
targets, intervention targets, and declaration uniqueness are unchecked → silent
wrong cell / phantom compartment / last-wins overwrite.

**Broader fix:** ONE reference-integrity pass (both sides of the IR contract)
driven by an exhaustive `fold_model_refs` so adding a ref-bearing field forces a
compile error in the traversal; route every name construction through the gh#111
dimension-aware resolver. **Meta-test:** `validate_rejects_malformed_ir` (a
parametrized table: under/over-indexed table, bare stratified init, duplicate
let, param/let collision, dangling balance/add target) + a coverage-completeness
meta-test (one dangling-ref fixture per field).

## RC5 — OCaml↔Rust mirrored logic with promised-but-missing cross-language goldens 🟢

**Retires:** gh#98 (`parse_iso_date` drift, dead `origin_rata_die`), gh#147
(model_hash omits origin/time_unit/schedule).

Calendar/time conversion is mirrored by hand across the IR boundary and the
contract is _documented_ as defended by a cross-language golden table — that
table does not exist. Model identity is hashed two ways (a hand-maintained
allowlist `hashing::model_hash` vs the whole-IR `resolve::model_digest`); the
allowlist omits exactly the fields that change the trajectory.

**Broader fix:** stand up `ir/golden/caltime.tsv` read by _both_ a Rust and an
OCaml test (leap rules, month boundaries, out-of-range must reject identically);
delete `hashing::model_hash` and route the `[design.*]` path through the single
whole-IR digest. **Meta-test:** the caltime equivalence golden itself + a TDD
red that `date("2020-02-30")` errors with a named code on both sides.

## RC6 — `dt` and the optimization objective are resolved from divergent sources 🟢

**Retires:** unified-time #12 (fit run ignores model `dt`), dossier #97
(profile-PMMH loglik≠params), #129 (survey ranks by likelihood not posterior).

`simulate` and `fit run` resolve the integrator `dt` differently (fit defaults
to 1.0, ignoring `simulate { dt }`); and "best point" reporting selects by
likelihood while the saved params/inits target the posterior — an incoherent
(loglik, θ) pairing under a non-flat prior.

**Broader fix:** one `resolve_effective_dt(model, cli_override)` used everywhere
(fit defaulting _from_ `model.simulation.dt`); one
`score_point(loglik, params,
priors) -> {loglik, log_prior, log_posterior}` so
rank-objective and saved-score are the same object. **Meta-test:** a model with
`simulate { dt=0.1 }` fits and re-simulates at the same dt; a non-flat-prior
property test that survey/profile select the posterior-max, not the
likelihood-max.

---

## Cross-cutting: the meta-tests that convert "passes" into "proven correct"

The recurring theme behind half the dossier is **tests that pass whether or not
the code is correct.** Beyond the per-RC oracles above, stand up:

- **value/grad consistency** (RC1) — `value_of(grad_fn) == value_fn` per
  fixture.
- **parameter-sensitivity / autodiff completeness** — for every estimated param,
  perturbation changes the loglik ⟺ the analytic gradient is nonzero (catches
  the silent `TimeFunc/TableLookup → Const 0.0` drop, #119/#8).
- **FD over the _full_ estimated vector**, not a hand-picked `params_to_check`
  (#5/#16/#20).
- **cross-backend agreement harness** over adversarial schedules (coincident
  events/interventions, two fire-times within one dt, sub-dt Recurring,
  multi-writer Set) — byte-identical counts (#3/#6/#95), and tight tolerances
  (#95's `rel<0.30` hides a 30% bias).
- **PF analytic-marginal oracle** (the `particle_filter.rs` docstring promises
  it; #13) + **recovery-asserts-truth** (the `tests/recovery` harness is manual
  / non-asserting).
- **malformed-IR rejection** table (RC4).

These are the "broader gaps in testing" — each is one meta-test that closes a
whole class, not a per-issue assertion.

---

## Recommended elevation

| RC                                         | turn into                                           | collision                                                      |
| ------------------------------------------ | --------------------------------------------------- | -------------------------------------------------------------- |
| RC2 capability gate                        | proposal exists (v2) — ready after review folded    | 🟢                                                             |
| RC1 value+grad single traversal            | proposal — highest inference-correctness leverage   | 🔴 coordinate w/ inference agent (it's the gh#197/#5/#20 home) |
| RC4 single validation pass + #111 resolver | proposal (gh#111 already the umbrella)              | 🟢                                                             |
| RC3 one action resolver + checked_count    | issue/small proposal                                | 🔴 effects.rs                                                  |
| RC5 cross-language goldens                 | gh#98/#147 already cover; add the golden-table task | 🟢                                                             |
| RC6 dt + score single-source               | two focused issues (dt; score_point)                | 🟢                                                             |
| meta-tests                                 | a testing-infrastructure issue (extends gh#179)     | 🟢 mostly                                                      |

---

## Execution tracker (green only; RC1/RC3 held — inference agent's, ~done)

Each green issue is fixed via a worktree-isolated worker → independent reviewer
(TDD red→green; patch at `/tmp/camdl-fixes/gh-NNN.patch`). Status updated as
patches land and pass review.

**Live status (round 2).** Waves 1+2 done: ✅ `#191+#192` and `#112` approved;
`#97 #114 #117 #123 #129 #147 #98` came back changes_needed (test tautological /
incomplete / new diagnostic regression / missing prior_hash check / etc.). Main
checkout cleaned to pristine HEAD (workers had leaked edits into it). The two
worktree blockers are fixed: a hardened isolation prompt, and the
`ir_version_generated.ml` codegen step (so `camdlc` builds in worktrees →
integration tests run there). A **rework wave** is in-flight, file-CLUSTERED to
avoid the shared-file conflicts (4 patches touched `expander.ml`, 2 touched
`validate.rs`) and with each reviewer's prior feedback folded in, producing
git BRANCHES (not loose patches):

- `fix/ocaml-validation` ← #112 + #117 + #114(ocaml) + #98 (defers #98's C7
  `origin_rata_die` — needs a schema bump)
- `fix/rust-validate` ← #123 + #114(rust)  ·  `fix/gh-97-profile-loglik` ← #97
- `fix/gh-129-survey-posterior` ← #129  ·  `fix/gh-147-cache-key` ← #147
  (minimal key fix + documents the deferred hit-path race; not the CasSink migration)

| issue | RC  | what                                                      | wave | status   |
| ----- | --- | --------------------------------------------------------- | ---- | -------- |
| #97   | RC6 | profile-PMMH `final_loglik` = map_loglik (drop `.max`)    | 1    | rework: fix correct, but red test tautological → needs the mle.toml-vs-pfilter integration red test |
| #112  | RC4 | table-lookup arity guard (E202)                           | 1    | ✅ approved (genuine red test) — re-verify on clean HEAD then land |
| #117  | RC4 | duplicate/cross-namespace name pass + comp→param→let      | 1    | rework: correct for primary repros, leaves a residual collision hole + deviates from dossier |
| #147  | RC5 | design-branch cache key includes t_end/cadence/origin/tu  | 1    | rework: closes the key half (genuine red tests) but leaves the hit-path race; decide CasSink-migrate vs document |
| #129  | RC6 | survey_top_k ranks by log-posterior, not likelihood       | 1    | rework: ranks correctly but OMITS the `prior_hash` cross-check → opens a new silent hole |
| #191+#192 | RC2 | one worker: grant chain_binomial BALANCE + non-blank name, AND wire the gate into fit-run per stage (merged — wiring an un-fixed gate would newly reject balance{}) | 2 | in-flight |
| #114  | RC4 | stratified-init membership check vs expanded compartments | 2    | in-flight |
| #123  | RC4 | ir::validate adds intervention/balance/init/table-arity ref checks (scoped, not the full fold framework — that's #111) | 2 | in-flight |
| #98   | RC5 | parse_iso_date range-validate + caltime.tsv cross-lang golden (origin_rata_die schema decision deferred) | 2 | in-flight |

**Held — proposal-grade or red (NOT in the green waves):**

- RC1 (#1, #5, #20, #79, #119-B) and RC3 (#4, #6, #14, #124, set(real,<0)) — 🔴
  inference agent's files; nearly done upstream.
- #111 (dimension-aware resolver) — L, RC4's structural umbrella; needs its own
  proposal. The localized #112/#114/#117/#123 land without it; #111 then collapses
  the remaining string-concat sites.
- RC2-full ExecMode consolidation — proposal v2 ready
  (`proposals/2026-06-08-capability-gate-consolidation.md`); the localized
  #191/#192 close the immediate issues; the full single-gate refactor (+ #15
  RUNTIME_DT, #119 PARAMETRIC_FORCING) lands via the proposal.
- #12 (param-TOML dimensions) — L; separate proposal.

**Process incident (worker isolation).** Wave-1/2 worktree workers, given the
dossier as an absolute *main-tree* path, edited code via absolute main-tree paths
too — leaving stray uncommitted edits in the main checkout (`expander.ml`,
`test_compiler.ml`, `methods.rs`, `mod.rs`, `validate.rs`). Recoverable: the real
deliverables are `/tmp/camdl-fixes/*.patch`; main is cleaned to pristine HEAD
after the waves, and every patch is re-verified by applying to a clean HEAD +
running its test in isolation (this is also the gate against cross-contaminated
patches and the suspect in-worktree test runs). Also: `camdlc` won't build in a
fresh worktree (`Unbound module Ir_version_generated` — a generated file absent
from a checkout), so Rust *integration* tests needing camdlc can't run there;
OCaml `dune runtest` and Rust unit tests are fine. The rework wave uses a fixed
worker prompt: edit ONLY via worktree-relative paths; read the dossier
read-only; never write a `/Users/vsb/...` path.

**Demonstration plan (after all green land):** a verification pass that, per
retired issue, re-runs its red test against the merged tree to confirm green, and
emits a closeable-issues list (issue → test → pass) so the GH closures are
evidence-backed, not asserted. Candidate retirements once green lands:
#97, #112, #114, #117, #123, #129, #147, #191, #192, #98 (+ #93 already closed).

**Demonstration results — branch `integrate/green-fixes` (commit 79695743), clean main base.**
8 of 9 issue-groups demonstrate GREEN; #129 fails on a clean tree (the clean-base
re-verify caught what the contaminated-worktree worker+reviewer missed):

| issue | evidence (test → pass) | status |
| --- | --- | --- |
| #112 #114 #117 #98 | `cd ocaml && dune runtest` → 428 tests (E202 arity, E278 decl-names, E277 init, E223 dates, caltime 21) | ✅ green |
| #123 (+#114-rust) | `cargo test -p ir` → 22 (balance/intervention/init target + table-arity rejects) | ✅ green |
| #98 cross-lang | `cargo test -p ir --test caltime_golden` → 1 (OCaml↔Rust parsers agree) | ✅ green |
| #191+#192 | `cargo test -p cli --bin camdl fit::methods` → 13 (accepts balance; rejects real-comp; non-blank name) | ✅ green |
| #147 | `cargo test -p cli --bin camdl hashing` → 35 | ✅ green |
| #97 | `cargo test -p cli --test profile_pmmh` → 3 incl `reported_loglik_matches_saved_mle_params` | ✅ green |
| **#129** | `survey_top_k_pgas` + `pmmh_bad_init_skip` → FAIL: `parameter 'beta' has no value` | ❌ held — focused rework |

**Closeable once `integrate/green-fixes` lands (8):** #97, #112, #114, #117,
#123, #147, #191, #192 (+ #98's two halves). **#129 held**: survey landscape.tsv
columns / init parser inconsistent on a clean base + the fixture pins a stale
`ir_version: "0.7"` — to be fixed + re-verified on clean main (worktree-green ≠
clean-green proved decisive). Added `rust/crates/ir/tests/caltime_golden.rs` (the
cross-language Rust reader C1a had omitted).

**FINAL — branch `land/green-8` (commit 4cb25b9e), clean fast-forward over
`origin/main`.** All 8 issue-groups GREEN on a clean tree; the clean-tree
re-verification found + fixed **two integration bugs the worker/reviewer
worktrees had masked** (their bases carried inherited commits):

1. **#191 gate regression** — `gate_run_stages_against_model` built a
   `CompiledModel` from the raw IR *before* estimated params resolve, so every
   `init = survey_top_k` / estimate-only fit died with "parameter 'beta' has no
   value". Fixed: fill value-less params with a placeholder for the *structural*
   capability scan (`required_capabilities()` ignores param values).
2. **#147 `model_hash` drift** — production `model_hash` correctly gained
   `origin`/`time_unit`/`output`/`simulation`; the survey tests'
   `model_hash_for_test` *reimplementation* (RC5 forked-source-of-truth, live)
   was stale → synced all three to mirror production.

Final demonstration (clean `origin/main` base): OCaml `dune runtest` 9 suites ·
`cargo test -p ir` 22+1 · cli `fit::methods` 13 · `hashing` 35 · `profile_pmmh`
3 · `survey_top_k_pgas`/`_pmmh`/`pmmh_bad_init_skip` 1 each — **all green**.

**#129 was INNOCENT** of the survey-init failures (the #191 gate was) — its
posterior-ranking rework is now unblocked; re-test on this fixed base. **Ready to
merge + close (8):** #97, #112, #114, #117, #123, #147, #191, #192 (+#98).

---

## Issue-count reduction campaign (2026-06-08)

Goal: drop the count via dedup + already-fixed closures (fast, no code), then an
S-class knockdown, leaving only the tricky tier.

**Merged + closed (green-8 batch):** #97, #98, #112, #114, #117, #123, #147,
#192 (#191 reopened — only the interim gate landed). On `origin/main` (e8a30ca8).

**Dedup + already-fixed pass — closed 18 (92 → 74):**
- Dup: **#175 → #146** (hierarchical PGAS+NUTS Gate-3b; #175 is the user-facing repro).
- Already-fixed (verified the load-bearing artifact on main, then closed):
  #71, #80, #81, #94, #110, #113, #136, #145, #148, #152, #176, #181, #188,
  #193, #194, #196; **#100** closed as obsolete (the `[source.from_csv]` surface
  was never merged into this lineage).
- **Held #187** (claim that PGAS ignores scheduled interventions): triage says
  stale but verification was a soft CHECK — it's an inference-path correctness
  claim, so NOT closed without a real trace.

**S-class reliably-landable (triage found only 5 — the bar is strict):**
- In-flight batch 1: #66, #174, #108, #37 (`wmtgrg7bo`) → clean-verify then land+close.
- Batch 2 (queued, launch after batch 1 frees its worktrees): #36, #124, #128, #183.
- After both → S-class drained; ~66 open.

**Residue = the tricky tier (left for deliberate work):** M-class (~27: e.g.
#29, #55/#56, #96, #101, #103, #122, #125, #126, #127, #134, #156, #169, #177,
#179, #189, #190, #198, #199) and L/proposal/inference-owned (~41: RC2-full
consolidation, #111 resolver, #95 sampler, #119/#186 frozen-params, #197/#200
grad-drift [inference agent], #129 re-test, ABC #203, reactive #204, etc.).

