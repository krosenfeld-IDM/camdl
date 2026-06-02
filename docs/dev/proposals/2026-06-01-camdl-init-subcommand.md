# `camdl init` — scaffold a modeling project

Status: proposed
Target: v0.3 CLI
Related: `2026-06-01-camdl-docs-subcommand.md` (the two compose — see "Composition with `camdl docs`")

## Problem

There is no canonical layout for a camdl *modeling project*. `docs/project-structure.md`
describes the camdl monorepo's own internals (ocaml/, rust/crates/, ir/golden/);
the book's getting-started uses a deliberately flat single-file layout; and real
projects each invent their own (one downstream project carries `model/` +
`data/processed/` + ad-hoc preprocessing scripts + a hand-written
`CONSTRUCTION_PLAN.md`). Users and agents alike spend their first hour deciding
where files go, and they decide inconsistently — sometimes badly (reaching for a
`tables {}` read to hold a scalar instead of a parameter, because no scaffold
steered them).

It also leaves the agent-discoverability gap open. camdl's usage guidance lives
in `AGENTS.md` *in the camdl repo* — an agent working in a user's project never
sees it. `camdl init` closes this at project-creation time by writing an
`AGENTS.md` that lands **auto-loaded** in the new project.

`camdl init` establishes the blessed layout, drops a working example so the
project runs immediately, and bakes in reproducibility (git, a CAS-aware
`.gitignore`, a Makefile for data steps). The UI layer is the right place to be
opinionated; this is appropriate, prescriptive rigidity on top of a flexible
core.

## Command surface

```
camdl init [DIR]                 # git semantics: bare → current dir; DIR → create & scaffold DIR
camdl init kano-measles --template spatial
camdl init --no-git
camdl init --with-analysis
```

| Flag | Effect |
| --- | --- |
| `--template <name>` | `default` (comparison), `spatial`, `minimal`. Default: `default`. |
| `--with-analysis` | also create an `analysis/` dir with a dependency-free README |
| `--no-git` | skip `git init` (still writes `.gitignore`) |
| `--force` | scaffold into a non-empty directory (default: refuse, error-as-feature) |

`camdl init` with no `DIR` scaffolds the current directory; `camdl init foo`
creates `foo/` and scaffolds it — exactly `git init`'s contract.

## Default template (`default`) — the comparison shape

Real epidemiological modeling is never "fit one structure and stop"; it becomes
"is seasonality worth it? Poisson or NegBin? does coupling help?" So the default
scaffolds for *comparison* — which scales **down** to one model and **up** to
many — rather than for the single-model starting point.

```
myproject/
├── AGENTS.md   README.md   .gitignore
├── Makefile                    # fit-all → score → compare → table
├── data/
│   └── cases.tsv               # shared data — every model fits this, identically
├── models/
│   ├── m1_seir.camdl           # working models — `camdl check` passes immediately
│   └── m2_seir_seasonal.camdl
├── experiments/                # fit configs (estimate/prior/fixed + stages + data ref)
│   ├── m1_seir.toml            #   AND any concrete --params value files (SBC truth, etc.)
│   └── m2_seir_seasonal.toml
└── results/                    # gitignored: content-addressed fit/sim outputs
```

Design decisions baked in:

- **`experiments/` is the single home for "things that define a run."** A
  fit.toml's `[estimate.X]` (bounds + prior) and `[fixed]` blocks **are** the
  parameter specification — so there is no separate `params/` directory. Fitted
  values come *out* into `results/`, not in by hand. The narrower case of a
  concrete `--params` value file (a *truth* file for synthetic-recovery / SBC, an
  extracted MLE) lives beside its config as `experiments/<name>.values.toml`.
- **`results/` is the CAS output root** (per `camdl-run-spec.md`) and is
  gitignored: outputs are content-addressed and regenerable, so committing them
  defeats the point and bloats the repo. Cite a fit *hash* in writeups, not the
  files.
- **Two working models, not one.** The project runs end-to-end out of the box
  (`make check` / `make fit-all` succeed before the user writes anything), and
  the second model demonstrates the comparison the layout exists for.

The Makefile is what makes this a *comparison project* rather than a folder of
models. Fit configs are `experiments/*.toml` excluding `*.values.toml`:

```make
MODELS  := $(wildcard models/*.camdl)
FITS    := $(filter-out %.values.toml,$(wildcard experiments/*.toml))

check:   ; @for m in $(MODELS); do camdl check $$m; done
fit-all: $(FITS:experiments/%.toml=results/.done-%)
results/.done-%: experiments/%.toml ; camdl fit run $< && @touch $@
table:   ; camdl fit table results/          # cross-fit diagnostics matrix (Â, ESS, gates)
compare: ; camdl compare results/…           # prequential predictive comparison
```

(Exact flags on `fit run` / `compare` / `fit table` are verified against the CLI
before the scaffold ships — see Testing.)

## Templates

`--template spatial` — one structurally-rich model whose real work is the data
pipeline. Foregrounds `data/raw → processed` (the committed-reference /
deterministic-processing split) and bakes in the "scalars are parameters
generated by preprocessing, not tables" lesson:

```
kano-measles/
├── AGENTS.md  README.md  .gitignore
├── Makefile                    # `make data` (raw→processed) is the heart
├── data/
│   ├── raw/                    # committed + provenance: census, contact survey, mobility
│   └── processed/              # gitignored: model-ready tables (pop, coupling W, contact)
├── scripts/                    # raw → processed; generates experiments/*.values.toml (vital rates)
├── models/  └── kano_seirv.camdl   # dimensions = lga × age; reads many tables
├── experiments/  └── kano.toml
└── results/                    # gitignored
```

`--template minimal` — flat, book-style, no git ceremony. For learning or a
throwaway exploration:

```
quickstart/
└── sir.camdl   params.toml   cases.tsv   .gitignore
```

All three share one skeleton dialed up or down; they are not separate
philosophies.

## The scaffolded `AGENTS.md` — thin, durable, points at `camdl docs`

The one real design decision. If `init` writes the full briefing into the user's
repo, that copy goes **stale** as camdl evolves — the user now owns a snapshot.
So the scaffolded `AGENTS.md` is short and *durable* (project-specific
orientation that rarely changes) and defers all evolving detail to `camdl docs`,
which is version-locked to the installed binary:

```markdown
# <project> — a camdl modeling project

Layout:
- models/        .camdl model files
- experiments/   fit configs (priors/bounds/stages) + any --params value files
- data/          observation data + tables
- results/       fit/sim outputs — content-addressed, gitignored

Workflow:  camdl check → simulate → survey → fit run → fit summary
Compare:   fit each model in experiments/, then `make compare`

For current, version-matched guidance, run:
  camdl docs agents      # orientation, idioms, when to ask the human
  camdl docs workflow    # the canonical fit workflow
  camdl docs example <name>   # a working model to copy

Reproducibility: outputs are content-addressed. Cite a fit hash
(`camdl fit where experiments/<x>.toml`) in writeups; a reader with the
source reproduces bit-for-bit.
```

Project-specific orientation stays in the file; the evolving how-to lives in
`camdl docs`. Neither duplicates the other, and the stale-copy problem never
arises.

## git + `.gitignore`

`git init` runs by default (`--no-git` opts out). camdl's reproducibility story
is "cite a fit hash, a reader clones the source, reproduces bit-for-bit" — that
requires the project be a git repo, and the `.gitignore` is essential or the
first `camdl fit` floods the repo with the regenerable `results/` tree. The
scaffolded `.gitignore` covers `results/`, `data/processed/` (where a template
uses it), and editor/OS noise. Committed: `models/`, `experiments/`, `data/raw/`
(+ provenance), `Makefile`, `AGENTS.md`, `README.md`.

## Composition with `camdl docs`

The two onboarding subcommands close the discoverability gap from both ends:
`camdl init` gives a *fresh* project an auto-loaded thin orientation; `camdl docs`
serves the *version-locked detail* (and reaches projects not created via `init`).
The scaffolded `AGENTS.md` is the seam — it is the auto-loaded pointer that sends
the agent to `camdl docs`.

## Sequencing & conflict surface

`camdl init` is a new subcommand → it adds a `Command::Init` variant in
`rust/crates/cli/src/main.rs` plus a module, the **same** conflict surface as
`camdl docs` against the in-flight CAS run-input work. So implementation defers /
coordinates with that merge; this proposal is the record until then.

The scaffold's file *contents* (the example models, the Makefile, the AGENTS.md
template) are embedded via `include_str!`, the same mechanism as `camdl docs` —
which means they are subject to the same **no-stale-command CI gate**: every
`camdl …` line in a scaffolded Makefile/README must parse against the live CLI,
and every scaffolded `.camdl` must pass `camdl check`. A project `camdl init`
creates can never ship broken.

## Testing

- `camdl init` into a temp dir produces a tree that `camdl check`s clean and
  whose `make fit-all` runs to completion on the example.
- Every scaffolded `camdl …` command parses against the live CLI (shared gate
  with `camdl docs`).
- `--no-git` omits `.git` but keeps `.gitignore`; `init` into a non-empty dir
  errors without `--force`.
- Each `--template` snapshot-tested for its file set.

## Future / pins

- **Model composition.** Structural comparison at scale wants a shared base +
  structural deltas, but the DSL has no includes/modules — so today many
  variants are many full `.camdl` files (or a `scripts/` generator, as `spatial`
  shows). A DSL composition mechanism is a separate, larger question; pinned
  here because `comparison` is where it bites.
- **More templates** — `forecasting` (rolling refits as data arrives) and
  `paper` (reproducible-study analysis layout) are plausible later additions;
  left out of v1 to keep the set small.
- **Sourced examples.** The starter models can be drawn from the same curated
  `ocaml/golden/` set that backs `camdl docs example`, so the two subcommands
  share one example corpus.
