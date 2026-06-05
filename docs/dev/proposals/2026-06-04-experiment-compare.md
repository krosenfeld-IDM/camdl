# Experiment files and the `compare` block

Status: Proposed — not implemented. The surface below was previously sketched in
the language spec (§17.4/§17.5) but never existed in the grammar; it was removed
from the spec on 2026-06-04 and preserved here so the design isn't lost.

## Problem

Counterfactual policy questions are paired: "how many cases does this SIA avert,
relative to no SIA?" Answering them means running two scenarios of the _same_
model under _matched_ randomness and reporting a cross-scenario derived quantity
(cases averted, relative reduction). Today camdl supports single-model scenarios
(`scenarios {}`, live — patch/enable/set/scale/ compose, spec §17.1–17.3), and
the CLI can run one scenario at a time, but there is no first-class construct
for _paired_ runs or for cross-scenario quantities.

## Proposed surface

An external experiment file binds a model + params and declares scenarios plus a
`compare` block:

```camdl
experiment("Nigeria SIA evaluation") {
  model  = "models/seir_nigeria.camdl"
  params = "params/fitted_2024.toml"

  scenarios {
    with_sia    { enable = [sia_round_1] }
    delayed_sia { enable = [sia_round_1]
                  set    = { sia_time = 365 'days } }
  }

  compare {
    pairs = [
      (baseline, with_sia),
      (baseline, delayed_sia)
    ]
    seeds = 1 to 1000    # range syntax → integers 1..1000
  }
}
```

## Semantics

The `compare` block drives paired scenario simulation with matched seeds:

- `pairs` lists 2-tuples `(reference_scenario, test_scenario)`. The keyword
  `baseline` is the identity patch (no modifications).
- `seeds = N to M` generates integers N, N+1, …, M.
- For each (pair, seed), both scenarios run with the same seed. Because the
  runtime uses a stateful PRNG, pre-divergence coupling (common random numbers)
  holds only while both runs consume the RNG in the same order — the paired-seed
  CRN property already documented for `enable`/`disable` scenarios. This is the
  load-bearing reason to run the pair together rather than independently.

## Open dependency: cross-scenario derived quantities

The original sketch expressed cross-scenario quantities through an
experiment-level summary block:

```camdl
output {
  summary {
    cases_averted      = baseline.total_cases - scenario.total_cases
    relative_reduction = cases_averted / baseline.total_cases
  }
}
```

with `baseline.QUANTITY` / `scenario.QUANTITY` resolving against per-run summary
scalars. That depends on a **per-run summary/reduction surface**
(`peak_I = max(I)`, `total_cases = cumulative(...)`, etc.) which does not exist
— the `output { summary {} }` stub was parsed-but-dropped and was removed from
the frontend on 2026-06-04. So implementing `compare` requires first deciding
where cross-scenario reductions live. Options:

1. Build a real per-run summary surface (temporal reductions over the
   trajectory: max/min/cumulative/at), then let experiment output reference
   `baseline.X` / `scenario.X`. This is also the natural target surface for a
   future calibration-to-summaries / probe-matching inference mode (ABC,
   synthetic likelihood) — see the malaria notes — so it has independent value.
2. Compute cross-scenario quantities downstream from the two runs' emitted
   trajectories/observations (CLI- or notebook-side), keeping `compare` purely
   about _producing the paired runs_ and leaving the arithmetic to the analyst.
   Smaller surface; defers the reduction-language question.

Recommendation: option 2 first (paired-run production is the hard,
RNG-coupling-sensitive part and is independently useful), with the summary
surface (option 1) as a separate lift if/when calibration-to-summaries is on the
roadmap.

## Relation to existing surface

- Single-model `scenarios {}` (patches, `extends`, expression scope) is
  implemented and stays — spec §17.1–17.3.
- The CAS run-identity work (gh#147) already keys runs by content hash,
  including scenario identity; a `compare` driver would produce a set of CAS
  leaves (one per pair×seed×scenario) that downstream tooling can enumerate.
- Paired-seed CRN coupling is an existing, tested property of the runtime, not
  new work for this feature.

## Why deferred

The whole construct was spec-ahead-of-code: no `experiment`/`compare` rule ever
existed in `parser.mly`/`ast.ml`. Documenting an unimplemented feature in the
normative spec misleads both human authors and coding agents, so the text was
removed. When this is picked up, implement against this proposal (grammar →
expander → a `camdl experiment` / `compare` driver), decide the
cross-scenario-quantity question above, and add golden coverage for the
paired-seed coupling.
