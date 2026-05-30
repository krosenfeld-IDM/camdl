# Benchmarking & profiling camdl

In-context how-to for the scaling/perf machinery. The *why* and the staged
results live in the investigation note
[`docs/dev/notes/2026-05-29-foi-scaling-bench.md`](../../../../docs/dev/notes/2026-05-29-foi-scaling-bench.md);
this file is the durable "how to run it." Keep it current, not chronological.

## The pieces

| piece | path | role |
| --- | --- | --- |
| model generator | `scripts/gen_scaling_models.py` | parametric toy SEIR spatial+age `.camdl` reproducing the Kano FOI shape. Knobs: `-P <patches> -A <ages> --coupling on\|off --grad full\|minimal -o out.camdl` |
| macro sweep | `scripts/bench_scaling.py` | full compile→simulate pipeline across a P×A grid → TSV (`sim_s` is parse-dominated at scale) |
| micro bench | `benches/scaling.rs` | criterion: `eval_propensities`, `step_one` (per-step), `load_parse_compile`. Fixtures are gitignored (`make bench-micro` regenerates) |

`make` targets: `bench-scaling`, `bench-micro`, `flamegraph-real`,
`flamegraph-bench`. Profiling uses the `[profile.profiling]` build (release +
symbols); tools: `cargo install inferno samply`.

## Forward `simulate` flamegraph (exists)

`make flamegraph-real` builds the profiling binary, generates the
P=44,A=21,coupling=on,grad=full anchor (≈ the Kano model), and samples
`simulate` → `docs/dev/notes/assets/scaling/flamegraph_real.svg`. For
interactive: `samply record -- rust/target/profiling/camdl simulate <ir> …`.

## PMMH profiling (planned — `make profile-pmmh` to add)

Goal: find where a PMMH step spends time on large models. PMMH is the cost
Fix B did **not** touch — it shrank the IR (parse/load/one-shot simulate), but
the per-step particle-filter loop is unchanged. This profile is the evidence
that decides the next optimization.

Key facts that shape the harness:

- **PMMH is particle-filter-based — no gradients.** Use **`--grad minimal`**:
  ~5× smaller IR, and it keeps the run out of the OOM regime (the 2026-05-29
  crash was full-grad IR *materialization at compile*; minimal-grad + the
  Fix-B compiler bounds it). Particle-filter memory is just `N_particles ×
  state` — tiny even at Kano scale. So profiling PMMH at large P is **safe**,
  unlike full-grad simulate benchmarking.
- **Invocation** (template: `crates/cli/tests/profile_pmmh.rs`):
  `camdl profile --algorithm pmmh --pmmh-steps N --pmmh-particles M --pmmh-rho ρ
  <model.ir.json> --data <synth.tsv>` (set `CAMDL_SKIP_VERSION_CHECK=1`).
- **Synthetic data**: simulate the generated model once, extract weekly cases
  as `--data` (the `synth_weekly_cases_tsv` pattern in that test).

Pipeline for the target:

1. `scripts/gen_scaling_models.py -P{8,16,32,44} -A{1,7} --coupling on --grad minimal`
2. compile once (Fix-B compiler, minimal grad → small IR)
3. simulate once → synthetic `--data`
4. `camdl profile --algorithm pmmh` under `samply`, bounded steps/particles
5. flamegraph + per-stage wall breakdown across the P/A sweep

What to attribute: a step ≈ `N_particles × T_obs × (propensity eval + state
update + obs log-lik) + resample + proposal`. Expect the per-step propensity
eval (O(P²·A) with coupling) to dominate at large P → motivates the per-step
binding cache (the once-per-step preamble Fix B skipped) and/or particle
batching. Optionally wrap the run in `ulimit -v` per the memory-guardrail RFC.

### Results

_(none yet — fill in once `make profile-pmmh` runs.)_

## See also

- Figures & raw data: [`docs/dev/notes/assets/scaling/README.md`](../../../../docs/dev/notes/assets/scaling/README.md)
- Memory-guardrail RFC (why large compiles can OOM the host):
  [`docs/dev/proposals/2026-05-29-compiler-memory-guardrail.md`](../../../../docs/dev/proposals/2026-05-29-compiler-memory-guardrail.md)
