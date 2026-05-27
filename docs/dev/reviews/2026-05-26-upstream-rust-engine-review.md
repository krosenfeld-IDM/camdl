---
status: open
date: 2026-05-26
kind: upstream review
scope: Rust runtime engine — IR consumption, stochastic stepping, interventions/events, real compartments, observations, parameterized forcing/table semantics
reviewer: external / upstream
methodology: static code audit; cargo not available in reviewer container; full inference audit deferred
counts: 6 Critical / 9 High / 2 Medium + 1 structural cross-cutting fix
comparison: 2026-05-26-week-audit-engine-comparison.md
---

# Upstream Rust engine review — 2026-05-26

Reviewed the Rust runtime engine layer against the language spec and the compiler findings. Treated the spec as the semantic contract for runtime behavior, especially flat IR consumption, stochastic stepping, interventions/events, real compartments, observations, and parameterized forcing/table semantics.

I could not run the Rust test suite because `cargo` is not installed in this container:

```text
cd /mnt/data/review-engine/camdl/rust/crates/sim && cargo test --no-default-features
bash: cargo: command not found
```

So this is a static audit. I did not do a full inference audit yet, but I inspected the engine-adjacent observation/scoring paths where they share runtime state.

# Critical findings

## 1. Parameterized tables and forcing functions are frozen at model construction

**Location** — `rust/crates/sim/src/compiled_model.rs:561-681`; `rust/crates/sim/src/propensity.rs:182-187`; `rust/crates/sim/src/resolved_expr.rs:331-367`

**Category** — not wired through; numerical correctness; statistical correctness

**Defect** — `CompiledModel::new` evaluates inline table entries and all time-function fields once using `default_params`. Later simulation and inference calls pass a current `params` slice, but table values and forcing values no longer read from it. Any parameter used inside a table or forcing is baked into `table_values_cache` / `time_func_cache`.

**Why it matters** — The spec explicitly allows parameterized table entries and forcing arguments so users can infer seasonal amplitude, reporting ramps, spatial/contact matrix entries, or rate baselines. With the current runtime, IF2/PMMH/PGAS can propose new values for those parameters and the trajectory/likelihood will still use the construction-time defaults. The posterior then compensates through other parameters and silently targets the wrong model.

**Fix** — Do not store parameter-dependent table/forcing values as `f64` caches. Store them as `ResolvedExpr` trees and evaluate with the current `params` in `EvalCtx`. Split fast paths explicitly:

```rust
enum CompiledTableValues {
    Constant(Vec<f64>),
    Parametric(Vec<ResolvedExpr>),
}

enum CompiledTimeFuncField {
    Constant(f64),
    Parametric(ResolvedExpr),
}
```

Evaluate `Parametric` fields per parameter vector, or cache them behind a parameter-vector version key. Add tests where changing `amplitude`, `phase`, `baseline`, and a table-entry parameter changes both simulation trajectories and observation likelihoods without rebuilding `CompiledModel`.

**Severity** — Critical

## 2. Chain-binomial transition rates read zero for real-valued compartments

**Location** — `rust/crates/sim/src/chain_binomial.rs:24-29`, `61-68`, `178-191`, `261-283`; `rust/crates/sim/src/inference/chain_binomial_process.rs:64-70`, `91-98`

**Category** — numerical correctness; statistical correctness

**Defect** — `run_chain_binomial_with_observer` evolves `real_s` with RK4, but `step_one` receives only integer counts. `step_one` evaluates propensities against `scratch.real_s`, which is initialized to zeros and never populated from the actual real state. The inference process drops real state entirely by constructing `ParticleState` only from integer counts.

**Why it matters** — Any model where a stochastic transition depends on a real compartment is wrong under chain-binomial. The worked cholera/environmental-reservoir pattern `infection : S --> I @ beta_W * W / (K + W)` will use `W = 0` in all transition rates even while output shows `W` evolving. In inference, the likelihood is for a model where the environmental coupling is effectively absent.

**Fix** — Change `step_one` to accept real state:

```rust
pub fn step_one(
    model: &CompiledModel,
    counts: &mut [i64],
    real_values: &[f64],
    ...
)
```

Populate `scratch.real_s.values` from `real_values` before `eval_propensities`. Extend `ParticleState` to carry `real_values` or remove `REAL_COMPARTMENTS` support from `ChainBinomialSim` and `ChainBinomialProcess` until real-state propagation is implemented through inference. Add a regression where `W > 0` produces infections and `W = 0` does not.

**Severity** — Critical

## 3. Gillespie is not exact for time-dependent, forcing-dependent, or real-compartment-dependent rates

**Location** — `rust/crates/sim/src/gillespie.rs:177-181`, `193-225`, `256-260`, `301-339`; `rust/crates/sim/src/compiled_model.rs:122-165`, `178-192`

**Category** — statistical correctness; numerical correctness

**Defect** — Gillespie draws the next event time as `Exp(lambda_total)` using the current total propensity. That is only exact when propensities are constant between events. camdl rates can depend on `t`, `TimeFunc`, and real compartments evolved by ODEs. The code only recomputes time-dependent transitions at output/intervention boundaries and after integer events; it does not solve the integrated hazard and it does not track real-compartment dependencies at all.

**Why it matters** — Seasonal forcing, reporting ramps, policy-driven time functions, and environmental reservoirs produce continuously changing hazards. Event times become biased. Worse, output frequency can change the simulated dynamics because output boundaries trigger propensity recomputation. A model run with daily output and the same seed can follow a different stochastic law than the same model with weekly output.

**Fix** — Either reject Gillespie for any transition whose rate contains `Time`, `TimeFunc`, or a real compartment, or implement a correct nonhomogeneous SSA/PDMP algorithm using thinning or integrated hazards. Also add real-compartment dependency tracking:

```rust
real_comp_to_transitions: Vec<Vec<usize>>
real_dep_transitions: Vec<usize>
```

Until that exists, remove `REAL_COMPARTMENTS` from `GillespieSim::capabilities`.

**Severity** — Critical

## 4. Multi-source transitions are bounded by only the first source in fixed-step backends

**Location** — `rust/crates/sim/src/compiled_model.rs:536-541`; `rust/crates/sim/src/chain_binomial.rs:317-397`; `rust/crates/sim/src/tau_leap.rs:174-226`

**Category** — statistical correctness; numerical correctness

**Defect** — `source_groups` chooses the first negative stoichiometry entry as the source. For `A + B --> C`, the chain-binomial/tau-leap competing-risk draw is bounded only by `A`, then applies deltas to both `A` and `B`. Secondary sources can be over-consumed, or the particle/run errors after the fact with a negative compartment. The stochastic law is not the atomic multi-source transition promised by the spec.

**Why it matters** — This is not an edge case. Vector-host transmission, pair formation, chemistry-style reactions, and some migration/contact processes use multi-source transitions. If `B` is rare and `A` is abundant, the backend can draw more events than `B` can support, reject otherwise plausible particles, or bias rates downward because high-rate regions collapse.

**Fix** — Split transition handling by stoichiometry shape:

```rust
enum SourceShape {
    Inflow,
    SingleSource { src: usize },
    MultiSource { sources: Vec<(usize, u32)> },
}
```

Use Euler-multinomial source grouping only for `SingleSource`. For `MultiSource`, either implement a bounded multi-reactant draw with rejection/adaptive substepping or hard-error in tau-leap/chain-binomial and require Gillespie until a correct approximation exists. Add a test where `A = 1000`, `B = 1`, and `A + B --> C` cannot fire more than once in a step.

**Severity** — Critical

## 5. `deterministic(rate)` source transitions are silently skipped

**Location** — `rust/crates/sim/src/chain_binomial.rs:327-331`, `401-409`; `rust/crates/sim/src/tau_leap.rs:183-191`, `231-238`

**Category** — numerical correctness; user footgun

**Defect** — In source-group handling, deterministic transitions are marked `handled` and skipped with a comment saying they are handled separately below. The later ungrouped loop skips all `handled` transitions, so sourced deterministic transitions never fire. Deterministic inflows still work because they are not source-grouped.

**Why it matters** — A user can write deterministic aging, deterministic demographic flow, deterministic recovery, or deterministic waning and get a transition that compiles but has zero effect. This changes population structure and epidemic timing with no warning.

**Fix** — Either forbid `DrawMethod::Deterministic` on transitions with negative stoichiometry, or process deterministic sourced transitions explicitly before/after stochastic source groups with documented semantics. If supported, add their `round(rate * dt)` deltas to `pending_deltas` and test both `S --> I @ deterministic(...)` and `--> S @ deterministic(...)`.

**Severity** — Critical

## 6. Events and interventions silently no-op, clamp invalid policy values, and accept negative transfers

**Location** — `rust/crates/sim/src/intervention.rs:117-190`, `250-354`

**Category** — user footgun; not wired through; numerical correctness

**Defect** — `inject_event_deltas` ignores unknown compartments and non-integer targets. `apply_intervention` errors on unknown names but silently does nothing for mixed integer/real transfers. `FractionTransfer` clamps fractions to `[0,1]` instead of rejecting invalid coverage. `AbsoluteTransfer` accepts negative counts, so a negative transfer reverses the sign of the deltas. Always-active events also cannot affect real compartments.

**Why it matters** — Scenarios are public-health counterfactuals. A typo in a seeding event can be ignored. `fraction = 1.2` becomes 100% coverage instead of a hard error. `count = -10` can move people in the wrong direction. A vaccination or importation scenario can appear active while running as baseline or with a distorted intervention.

**Fix** — Validate every action before simulation:

```rust
validate_action_targets(model)
validate_action_domains(params/current_expr_values)
```

Rules:

* all action compartments must exist
* transfer endpoints must have the same compartment kind
* fractions must be finite and in `[0,1]`
* absolute counts must be finite and nonnegative
* integer actions must be integer-compatible after rounding policy is applied
* real-target events must either be implemented or rejected

Remove `clamp` from intervention semantics. Invalid policy values must be hard errors.

**Severity** — Critical

# High findings

## 7. The Rust IR validator does not validate most runtime-relevant references

**Location** — `rust/crates/ir/src/validate.rs:55-166`, `142-160`, `175-184`, `219-225`

**Category** — FFI; not wired through; type/trait design

**Defect** — The validator checks some transition, ODE, and likelihood references, but it does not validate initial-condition keys/expressions, intervention action targets/expressions, schedules, balance expressions, time-function expressions, table value expressions, overdispersion expressions, `rate_grad`, or output expressions. It also does not detect duplicate table or time-function names. `Projected` outside likelihood context is explicitly ignored.

**Why it matters** — The frontend compiler is not a sufficient trust boundary. The runtime consumes IR and must reject malformed IR before simulation. Several compiler findings from the previous pass become dangerous precisely because the Rust validator does not catch them: invalid init keys, bad scenario/intervention references, malformed tables, and projection mistakes can reach simulation and become no-ops, panics, or wrong trajectories.

**Fix** — Make `ir::validate` a complete schema+semantic validator. It must walk every `Expr` location in the model and validate every name against the correct namespace. Add specific passes for:

```rust
validate_initial_conditions
validate_interventions_and_events
validate_schedules
validate_balance
validate_tables
validate_time_functions
validate_observations
validate_output
validate_transition_draw_methods
validate_rate_grad_keys
```

`Expr::Projected` should be an error everywhere except likelihood argument expressions.

**Severity** — High

## 8. Initial conditions can be negative, fractional, non-finite, or truncated silently

**Location** — `rust/crates/sim/src/compiled_model.rs:935-963`

**Category** — user footgun; numerical correctness

**Defect** — Explicit integer initial conditions use `*val as i64`, which truncates and uses Rust float-to-int casting semantics. Parameterized integer initial conditions use `v.round() as i64`. There is no finite check, nonnegative check, integer-compatibility check, or domain diagnostic.

**Why it matters** — Initial conditions determine the epidemic seed. `I0 = NaN`, `I0 = -3`, `I0 = 0.6`, or a bad expression can become a plausible integer state instead of a hard error. This is exactly the "model runs but starts in the wrong population" failure mode that produces convincing but useless output.

**Fix** — Add checked conversion:

```rust
fn checked_int_initial_value(name: &str, v: f64) -> Result<i64, SimError> {
    if !v.is_finite() { error }
    if v < 0.0 { error }
    if (v - v.round()).abs() > 1e-9 { error }
    Ok(v.round() as i64)
}
```

For real compartments, require finite values. Move this into IR/runtime validation so the invariant is enforced before any backend runs.

**Severity** — High

## 9. Chain-binomial output can stamp a future state at an earlier requested output time

**Location** — `rust/crates/sim/src/chain_binomial.rs:165-226`; `rust/crates/sim/src/output.rs:4-17`

**Category** — numerical correctness; user footgun

**Defect** — Chain-binomial steps only split at `t_end`; it does not split at output times. After stepping from `t = 0` to `t = 1`, it emits any output times `<= 1` using the post-step state. If the schedule asks for `t = 0.5`, the snapshot is labeled `0.5` but contains the state at `1.0`.

**Why it matters** — Synthetic observations and trajectory outputs are time-indexed. If observation/output times are not exact multiples of `dt`, the likelihood or synthetic data can use the wrong state. This biases incidence timing and can move intervention effects across reporting windows.

**Fix** — Either require all output and observation times to align with `dt` for chain-binomial, or split chain-binomial steps at the next output/intervention/observation boundary like tau-leap does. If alignment is required, validate it before simulation and emit a hard error naming the first offending time.

**Severity** — High

## 10. Schedule and time-step validation is debug-only or missing

**Location** — `rust/crates/sim/src/time.rs:30-60`; `rust/crates/sim/src/output.rs:4-17`; `rust/crates/sim/src/intervention.rs:46-64`; `rust/crates/sim/src/config.rs:1-34`

**Category** — user footgun; numerical correctness

**Defect** — `time_to_step` and `interval_steps` use `debug_assert!` for finite times and positive `dt`, so release builds do not enforce those invariants. `output_times` loops forever for `step <= 0`. Recurring intervention schedules loop forever for `period <= 0`. `AtTimes` are cloned without normalization or finite checks.

**Why it matters** — A bad config should not hang a public-health run or emit a partial trajectory. In inference, one invalid schedule value from a parameter proposal can stall a worker instead of producing a controlled `-inf` particle or a named setup error.

**Fix** — Add constructors/validators for every runtime config and schedule:

```rust
SimConfig::validate()
OutputSchedule::validate()
InterventionSchedule::validate()
```

Require finite `t_start`, `t_end`, `dt`; `t_end >= t_start`; positive `dt`; positive output step; positive recurrence period; finite sorted/deduped `AtTimes`.

**Severity** — High

## 11. Tau-leap and ODE pass configured `dt` into expressions on truncated substeps

**Location** — `rust/crates/sim/src/tau_leap.rs:103-143`; `rust/crates/sim/src/ode.rs:198-238`

**Category** — numerical correctness

**Defect** — Both backends compute an actual truncated substep `dt = cfg.dt.min(next_boundary - t)`, but then pass `cfg.dt` into `eval_propensities` / `EvalCtx` in several places. ODE flow accumulation also evaluates propensities with `cfg.dt` while multiplying by actual `dt`.

**Why it matters** — The DSL exposes `dt`. Any rate expression, overdispersion expression, event expression, or diagnostic expression that references `dt` will see the wrong value on boundary steps. Boundary steps happen at outputs, interventions, and simulation end, so this can shift flows around observation windows and alter overdispersion.

**Fix** — Always pass the actual substep `dt` into `EvalCtx`:

```rust
eval_propensities(model, &int_s, &real_s, params, t, dt, &mut propensities)
```

If the configured nominal step is needed, add a separate field such as `configured_dt`; do not overload `Expr::Dt`.

**Severity** — High

## 12. Table lookup can panic in non-test runtime paths

**Location** — `rust/crates/sim/src/resolved_expr.rs:335-367`; `rust/crates/sim/src/propensity.rs:295-315`

**Category** — numerical correctness; user footgun

**Defect** — The fast resolved-expression evaluator panics on `OobPolicy::Error` when an index is out of range. `OobPolicy::Clamp` also computes `idx.clamp(0, n - 1)` without checking `n == 0`; an empty table makes the clamp bounds invalid. These are user- and particle-triggerable runtime paths.

**Why it matters** — A dynamic table index can go out of range under an inference proposal. One bad particle should not panic the entire process. It should either be rejected as a structural model error or converted into a controlled per-particle failure. Empty tables should be rejected at construction.

**Fix** — Validate `cached.len() > 0` for every table during `CompiledModel::new`. Make the fast evaluator fallible, or return a checked sentinel that the caller converts to `SimError::TableLookup`. Remove `panic!` from `eval_resolved`.

**Severity** — High

## 13. Observation projection and likelihood evaluation use a zero real state

**Location** — `rust/crates/sim/src/inference/multi_stream_obs.rs:232-239`, `305-312`, `320-359`, `376-450`; `rust/crates/sim/src/inference/chain_binomial_process.rs:64-70`

**Category** — statistical correctness

**Defect** — `MultiStreamObsModel` stores `real_s = RealState::new(...)` and passes that zero state into derived projections and likelihood expressions. `ParticleState` carries only integer counts, so inference cannot score observations that depend on real compartments.

**Why it matters** — A surveillance model can observe an environmental reservoir, wastewater concentration, mosquito abundance, or another real-valued state. `projected = W` or `mean = rho * W` will score as if `W = 0`. That pushes posterior mass toward compensating parameters or makes the likelihood degenerate.

**Fix** — Extend particle state and observation APIs to carry real state:

```rust
struct ParticleState {
    counts: Vec<i64>,
    real_values: Vec<f64>,
    flow_accumulators: Vec<u64>,
}
```

Then thread `real_values` into `eval_stream_projection`, `eval_likelihood_resolved`, `sample_obs_resolved`, and `eval_obs_mean_resolved`. Until that is implemented, reject observation expressions that reference real compartments in inference.

**Severity** — High

## 14. Gillespie sparse updates clamp negative propensities to zero

**Location** — `rust/crates/sim/src/gillespie.rs:49-54`, `216-223`, `315-339`

**Category** — numerical correctness

**Defect** — Initial full propensity evaluation uses checked `eval_propensities`, but sparse Gillespie updates call `eval_one`, which does:

```rust
eval_resolved(...).max(0.0)
```

Negative propensities after an event or time update become zero instead of an error.

**Why it matters** — Invalid rate expressions are backend-dependent. Tau-leap/chain-binomial can reject negative rates, while Gillespie can silently turn a transition off. Near-threshold models using expressions like `1 - immunity` can hide a modeling error and bias dynamics exactly when rates are small.

**Fix** — Make `eval_one` return `Result<f64, SimError>` and use the same negative/NaN/Inf policy as `eval_propensities`. Propagate the error with transition name and time.

**Severity** — High

## 15. Unknown `rate_grad` keys are silently dropped

**Location** — `rust/crates/sim/src/compiled_model.rs:786-807`

**Category** — not wired through; statistical correctness

**Defect** — `rate_grads_indexed` uses `filter_map`, so a gradient entry for an unknown parameter name is discarded. The comment says this indicates malformed IR, but the code does not error.

**Why it matters** — Gradient-based algorithms can receive partial gradients and treat missing components as zero. NUTS or gradient diagnostics then operate on a different model than the simulator. A typo in a gradient key becomes a biased proposal, not a compiler/runtime error.

**Fix** — Replace `filter_map` with checked resolution:

```rust
let idx = param_index.get(name.as_str())
    .ok_or_else(|| SimError::Validation(format!(
        "transition '{}' has rate_grad for unknown parameter '{}'",
        tr.name, name
    )))?;
```

Add a validation pass that every `rate_grad` key is a declared parameter.

**Severity** — High

# Medium findings

## 16. Tests check multi-source conservation but not multi-source stochastic law

**Location** — `rust/crates/sim/tests/bimolecular_conservation.rs:1-123`

**Category** — tests

**Defect** — The bimolecular tests assert conservation invariants, but they do not test that fixed-step backends bound event counts by every source compartment or that the distribution matches the intended multi-source semantics. A model can pass conservation tests for parameter regimes where overshoot is rare and still be wrong when the secondary source is limiting.

**Why it matters** — The dangerous regime for vector-host and pair-formation transitions is exactly asymmetric abundance: one compartment large, the other small. That is the case current tests need to cover.

**Fix** — Add tests with `A = 1000`, `B = 1`, high `k`, and `dt = 1`. Assert that chain-binomial/tau-leap either hard-error as unsupported or never consume more than one `B`. Add a distributional oracle for low-rate `A + B --> C` against Gillespie over many replicates.

**Severity** — Medium

## 17. No regression tests cover parameter-dependent table/forcing invalidation

**Location** — absence across `rust/crates/sim/tests/*`

**Category** — tests

**Defect** — The runtime has tests for periodic forcing and expression evaluation, but no test asserts that changing a parameter after `CompiledModel::new` changes a forcing function or table entry.

**Why it matters** — This is the exact failure mode that corrupts inference over seasonal amplitude, contact matrices, or reporting functions.

**Fix** — Add two tests:

```rust
compiled_once_then_params_changed_time_func_changes()
compiled_once_then_params_changed_table_lookup_changes()
```

Both should construct one `CompiledModel`, run/evaluate with two different `params` slices, and assert different propensities/log-likelihoods.

**Severity** — Medium

# Structural fixes that remove several defects at once

The runtime needs a stronger compiled/evaluation boundary. Right now `CompiledModel` mixes immutable structure with parameter-specific numeric caches. That is the source of the frozen forcing/table bug and part of the observation bug. The correct shape is:

```rust
struct CompiledModel {
    structure: Arc<ModelStructure>,
    resolved: ResolvedModel,
}

struct EvaluationContext<'a> {
    model: &'a CompiledModel,
    params: &'a [f64],
    int_state: &'a IntState,
    real_state: &'a RealState,
    t: f64,
    dt: f64,
}
```

Any value that can depend on `params`, `state`, `t`, or `dt` must remain an expression or a cache keyed by those inputs. Construction-time `f64` caches are only valid for expressions proven constant.

The backend capability system also needs to become stricter. A backend should not advertise `REAL_COMPARTMENTS` unless real state affects transition rates, observations, and inference consistently. A backend should not accept multi-source transitions unless its step law is correct for multi-source stoichiometry. Rejecting unsupported combinations is much safer than running the wrong model.

The runtime IR validator should become the hard gate for all model execution. The compiler should prevent bad IR, but the Rust runtime is the final boundary before public-health output. Every unresolved name, malformed schedule, bad domain value, unsupported backend feature, and dangerous expression context should fail before the first trajectory is simulated.
