# Diagnostic catalog

Central index of every diagnostic the camdl compiler can emit, plus
its severity, category, and rationale. Severities are
`Error | Warning | Info` per `ocaml/lib/compiler/diagnostics.ml`.

When you add a new emit site (`Diagnostics.error`, `.warning`, `.info`,
or future `.lint`), add a one-line entry here. Reviewers should
reject any diagnostic emit-site that isn't in the catalog.

## Code namespaces

- **`E0xx` — meta / internal** (compiler bug-class; should be rare)
- **`E1xx` — parse / lex** (file-level syntax issues)
- **`E2xx` — semantic / scoping** (resolution, redeclarations, missing names)
- **`E3xx` — dimensional analysis** (rate vs flux, P/T mismatch)
- **`E4xx` — schedule / forcing / intervention** (wrong-shape recurring blocks, range parse errors)
- **`E6xx` — simulation config** (rejected before runtime)
- **`W1xx` — model-file warnings** (questionable but valid declarations)
- **`W2xx` — IR / compiler warnings** (suspicious but legal expressions)
- **`W3xx` — covariate / forcing warnings** (alignment, interpolation)
- **`I3xx` — dimensional-analysis info** (undetermined dimensions, etc.)
- **`L4xx` — lints** (semantically valid but discouraged patterns)

## Errors

(Errors block compilation. The list below is the current state;
specifics are documented at each emit site in `ocaml/lib/compiler/`.)

| Code | Category | Summary |
|---|---|---|
| E001 | meta | internal compiler error / unreachable |
| E100 | parse | undeclared name |
| E101 | parse | duplicate compartment |
| E102 | parse | duplicate parameter |
| E103 | parse | duplicate let binding |
| E104 | parse | reserved name used as identifier |
| E105 | parse | unknown unit suffix |
| E106 | parse | malformed range |
| E107 | parse | ambiguous unit literal after `/` |
| E108 | parse | malformed initial-condition expression |
| E109 | parse | unknown forcing function shape |
| E200–E221 | semantic | scoping / declaration / resolution errors (multiple variants); E221 = read() data-file header has too few columns for the table's index dimensions |
| E230–E276 | semantic | observation, balance, simulation-block validation |
| E300 | dimensional | transition rate has wrong dimension (e.g. per-capita where total propensity expected) |
| E310 | dimensional | misc dimensional mismatch |
| E401 | schedule | recurring block missing required field |
| E402–E408 | schedule | recurring/periodic block validation (period, on-list, alignment) |
| E600 | runtime config | rejected before backend dispatch |

## Warnings

| Code | Severity | Category | Summary |
|---|---|---|---|
| W100 | Warning | model-file | (compiler.ml:52) — questionable model-file construct |
| W103 | Warning | model-file | (expander.ml:3073) — questionable model-file construct |
| W200 | Warning | IR | (expander.ml:1595) — suspicious IR shape |
| W201 | Warning | IR | (expander.ml:268) — suspicious IR shape |
| W203 | Warning | IR | (expander.ml:2746) — suspicious IR shape |
| W301 | Warning | covariate | periodic range not aligned to step size |
| W310 | Warning | covariate | (expander.ml:3153) — covariate / interpolation issue |
| W311 | Warning | covariate | (expander.ml:534) — covariate / interpolation issue |

(The above table is a starting skeleton. Each row should be
expanded with a one-paragraph rationale documenting the failure
mode the warning catches. Future emit-site additions must update
this table in the same commit.)

## Lints

Lints are warnings that catch *semantically valid but discouraged*
patterns — code that compiles and runs but is likely a bug. They
share the diagnostic infrastructure with `Wxxx` warnings; the `Lxxx`
prefix marks them as lints rather than compiler-internal warnings,
which clarifies their intent for users (a lint is asking "did you
mean this?", not "this is suspicious internally").

| Code | Severity | Category | Summary |
|---|---|---|---|
| L401 | Warning | discretization | discretization-correction pattern uses fixed time literal — likely meant `dt` (gh#54) |
| L402 | Warning | dead-code | compartment declared but referenced nowhere — likely a leftover (gh#168) |

### L401 — fixed-time-literal in Euler-correction pattern

**Fires when:** the AST contains the shape `(1 - exp(-RATE * TIME_LITERAL))`
or `(1 - exp(-RATE * TIME_LITERAL)) / TIME_LITERAL`, where `RATE`
has dimension `T^-1` and `TIME_LITERAL` is a constant time-typed
expression (e.g. `1 'days`, `0.5 'years`) rather than the `dt`
primitive.

**Why:** This is the Euler-multinomial per-step transition-probability
template (pomp's csnippet uses it via `(1 - exp(-(γ+μ)*dt))/dt`).
Pinning the `τ` factor to a fixed time literal produces a model
correct only when the runtime integrator step (`config.dt`) equals
that literal. Any other dt produces a discretization-pinned bias —
gh#53 / gh#54 are the canonical real-world example: He et al. 2010
measles fit at sub-day dt diverged from pomp by 5862 + 12-22 nats
(cohort fire-step bug + this discretization pinning, respectively).

**Fix:** use the `dt` primitive — `(1 - exp(-RATE * dt)) / dt` is
dt-invariant in effective R0 and matches pomp's standard formulation.

**False positives:**
- Pure unit conversions like `mu_per_day = mu_per_year / 1 'years`
  do NOT match (no `exp(...)` wrapping).
- Half-life computations like `t_half = ln(2) / lambda` do NOT match
  (no time literal inside `exp`).

If the fixed time literal IS intentional (a model where the dt-1-day
discretization is the calibrated form, not a bug), v2's per-site
suppression syntax (gh#55) will let users silence the lint
explicitly. Until then, the lint fires; users can suppress at the
CLI level via gh#56's `--allow=L401` flag.

### L402 — dead compartment

**Fires when:** a compartment is declared in the `compartments` block
but its name is referenced *nowhere* in the rest of the model — not in
any transition (stoichiometry, `source`/`dest`, or rate expression),
ODE equation, intervention action, observation projection or
likelihood, model-level `let` binding, initial condition, the balance
constraint, the identity-tracked (lineage) set, or a time-function
definition.

**Why:** A compartment touched by none of these contributes nothing to
the dynamics, the observation model, or the initial state. It is almost
always a leftover from editing (a removed transition, a renamed state)
rather than an intentional inert pool. The model still compiles and
runs, so this is a lint (Warning), not an error.

**Fix:** remove the compartment, or wire it into a transition / init /
observation as intended.

**False positives (explicitly NOT flagged):** the reference scan is
comprehensive precisely to keep the false-positive rate at zero. A
compartment is live if it appears in *any* position above. In
particular:
- a compartment used only inside a `let` binding body
  (`let N = S + I + R`, with `R` nowhere else) is live;
- a compartment used only in an observation (`CurrentPop`,
  `CurrentPopSum`, or inside a `DerivedExpr` / likelihood expression)
  is live;
- a compartment used only as an initial-condition target is live.

`CumulativeFlow`'s string argument names a *flow / transition*, not a
compartment, and is deliberately excluded from the reference set — it
never keeps a compartment alive.

The lint lives in `ocaml/lib/ir/lint.ml` (`Lint.check_model`), mirroring
the Dimcheck pass, and is routed to a non-blocking `Diagnostics.warning`
by `compiler.ml`'s `run_lint` (run by both `camdlc compile` and
`camdlc check`).

## Future work

- **gh#55**: per-site lint suppression syntax (e.g. `#[allow(L401)]`
  attribute or `// camdl-allow: L401` comment). Lets model authors
  silence a lint at a specific source location with documented
  rationale.
- **gh#56**: CLI lint-policy knobs (`--allow=L401`, `--deny=L401`,
  `-Werror`). Depends on gh#55 for `--allow` semantics.

Both deferred from gh#54's v1 scope. The bare minimum here is the
catalog (this file) plus the L401 inline emit; structured lint
infrastructure follows when ≥ 3 lints have customers asking for
suppression.
