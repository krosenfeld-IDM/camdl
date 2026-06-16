# ODE incidence/prevalence oracle (gh#166 Phase B)

External validation that camdl's **augmented ODE flow** (Q1B: incidence
integrated by the same RK4 as the compartments) is *correct*, not merely
different from the old first-order Euler flow. This is the gate that justifies
moving every ODE golden's incidence in Phase B.

camdl's fixed-RK4 ODE backend is compared, at the output grid, against three
**independent adaptive integrators**, for **both prevalence and incidence**:

| reference            | tool                         | method  |
| -------------------- | ---------------------------- | ------- |
| `*__scipy_rk45.tsv`  | Python `scipy.integrate`     | RK45 (Dormand–Prince) |
| `*__scipy_lsoda.tsv` | Python `scipy.integrate`     | LSODA   |
| `*__desolve_lsoda.tsv` | R `deSolve`                | lsoda   |

Each reference solves the same canonical RHS augmented with one
cumulative-incidence variable per transition (`dc_i/dt = rate_i`) — exactly the
construct camdl's augmented flow implements — and emits per-interval incidence
(`inc_<transition>`) plus prevalence (compartment columns) at the daily grid.

Models (`models/`): SIR, SEIR, and a 2-stage-latency TB model with timescale
separation (fast progression vs per-decade reactivation), which exercises the
sub-unit slow-transition incidence regime. Each `.camdl` is compiled to
`.ir.json` with params baked via `camdlc --set`; **those same params are
hard-coded in the generators** — if they drift, the trajectories diverge and the
gate fails (it is self-checking against drift).

## Layout

```
models/   <m>.camdl + <m>.ir.json   (compiled, committed)
gen/      scipy_oracle.py, desolve_oracle.R, run.sh
ref/      <m>__<method>.tsv          (cached, committed — CI reads only these)
```

## CI

The Rust gate `rust/crates/sim/tests/ode_incidence_oracle.rs` reads only the
cached `ref/*.tsv` — **CI needs neither Python nor R**. Result: camdl's
incidence matches all three references to ≪0.1% of tolerance; prevalence matches
to within the ±0.5 integer-snapshot rounding.

## Regenerating (only after a model/param change)

```
bash gen/run.sh          # needs: uv (scipy) + Rscript (deSolve)
# then re-compile models if the .camdl changed:
#   camdlc models/<m>.camdl --set ... -o models/<m>.ir.json
```
