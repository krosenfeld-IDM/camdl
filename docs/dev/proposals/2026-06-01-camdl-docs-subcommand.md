# `camdl docs` — embedded, version-locked documentation subcommand

Status: proposed Target: v0.3 CLI

## Problem

camdl's best usage guidance lives in two files in this repo: `AGENTS.md` (the
agent briefing — canonical workflow, error/diagnostics tables, idioms) and
`docs/intro.md` (the modeling-language on-ramp). Both are excellent and both are
**invisible at the moment they are needed.**

When an agent helps a user build or fit a model, it runs in the _user's_ project
directory, not in this repo. Neither `AGENTS.md` nor `docs/` is on that
filesystem, and neither is auto-loaded. The current fallback (`AGENTS.md`
§"downstream project": shallow-clone the docs, or `WebFetch` the hosted copy)
has a bootstrap gap: the instruction to shallow-clone lives _inside_ the repo
the agent has not cloned. The only reader who can find "run
`git clone --sparse …`" is one already looking at the repo — i.e. the population
that does not need it. So in practice a fresh agent gets nothing, guesses the
DSL from pomp/Stan priors (the wrong analogues — `AGENTS.md:28`), and writes a
plausible-but-wrong model.

The fix is the pattern mature CLIs converged on for guidance that must travel
with the binary: embed topic-addressable docs in the executable and serve them
from a subcommand (`go help <topic>`, `git help <guide>`, `aws help topics`,
`kubectl explain`). They chose it for three reasons that are exactly our
constraints: it works **offline** (the recurring target user — a health-ministry
modeler in an under-resourced setting — may be poorly connected or air-gapped,
and agent sandboxes are often network-restricted); it is **version-locked** to
the binary (no skew between what the docs say and what `camdl` does); and it is
**discoverable from the tool itself** (`camdl --help`), not from a URL the user
must already know.

## Goals

1. A downstream agent or human with only the installed `camdl` binary can reach
   the modeling on-ramp and the fitting workflow with zero setup, no network,
   and no repo clone.
2. One command, two audiences, one source: machine-clean markdown for
   pipes/agents; rendered + syntax-highlighted output for humans at a terminal.
   The two presentations are projections of the same embedded bytes — they
   cannot diverge.
3. Docs are version-locked to the binary by construction (same mechanism and
   guarantee as `IR_VERSION`, `ir/src/envelope.rs:32`).
4. The DSL is highlighted using the **one** grammar already maintained for
   editors (`tree-sitter/`), not a second grammar.

## Non-goals

- **No network / `--web` fetch.** That reintroduces the problem this solves. The
  hosted-docs URL and the shallow-clone recipe stay documented _inside_ the
  `agents` topic, for the heavyweight case (sustained multi-day work where
  pinning the full `docs/` + `ocaml/golden/` corpus is genuinely worth it).
- **No `docs/` reorganization in this proposal.** Topics source from the
  existing files as-is; the finer `workflow`/`errors`/`diagnostics` split is a
  future refinement (see end).
- **No section-anchor addressing** (`topic#section`) in v1 — `docs search`
  covers most of that need.
- **Release-coupled doc updates are by design, not a limitation.** A doc typo
  needs a rebuild to ship — which is correct: the embedded doc should describe
  the _shipped_ binary, never drift ahead of it.

## Command grammar

```
camdl docs                      # index: topics + one-line summaries
camdl docs <topic>              # emit/render one topic
camdl docs search <query>       # ripgrep-style search across all topics
camdl docs example [<name>]     # list golden examples, or emit one .camdl
```

Flags (on `camdl docs` and where sensible the sub-verbs):

| Flag                                           | Effect                                                                                                  |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `--json`                                       | machine-readable topic index (for an agent enumerating topics)                                          |
| `--all`                                        | concatenate the full user-facing corpus to stdout (agent slurp: one call loads everything into context) |
| `--raw`                                        | force raw markdown even at a TTY (copy-paste / redirect)                                                |
| `--color <auto\|always\|never>`                | content color; default `auto`; honors `NO_COLOR`                                                        |
| `--no-pager` / `--pager <auto\|always\|never>` | paging control; honors `$PAGER`                                                                         |
| `--width <N>`                                  | wrap width for rendered markdown; default `$COLUMNS`                                                    |
| `--params`                                     | on `example`: also emit the example's `*.params.toml`                                                   |

Unknown topic or example name → nonzero exit with the valid set listed
(error-as-feature, per the repo's diagnostics philosophy). Topic slugs carry
**aliases** so an agent's first guess resolves (`fit`→`inference`,
`dsl`→`language`, `model`/`modeling`→`getting-started`).

## Output contract (the dual-audience split)

This is the core of the design. Default `--color auto` / `--pager auto` branches
on **`std::io::stdout().is_terminal()`**:

- **stdout is not a terminal** (pipe, redirect, agent, CI): emit **raw
  markdown**, zero ANSI, no pager. An agent receives clean markdown it can paste
  or parse; nothing to strip.
- **stdout is a terminal** (human): render markdown structure to ANSI,
  syntax-highlight `camdl` code fences, and page the result if it exceeds one
  screen.

```rust
enum Sink { Pipe, Tty }   // resolved once from stdout().is_terminal() + flags

fn resolve_sink(color: ColorMode, raw: bool) -> Sink {
    if raw || color == ColorMode::Never { return Sink::Pipe }
    if color == ColorMode::Always { return Sink::Tty }
    if std::env::var_os("NO_COLOR").is_some() { return Sink::Pipe }
    if std::io::stdout().is_terminal() { Sink::Tty } else { Sink::Pipe }
}
```

**Critical:** this gate is on **stdout**, not stderr. The existing
`style::enabled()` (`style.rs:42-48`) samples `stderr().is_terminal()` — correct
for diagnostics, wrong for _content_. Driving `docs` content off
`style::enabled()` would flood ANSI through `camdl docs inference | less`
(stderr is still a TTY) — the exact `show` bug
`2026-05-30-cli-io-and-progress-ux.md` (lines 461-465) fixes by gating on
`stdout.is_terminal()`. `camdl docs` adopts that same contract. The
`bold/cyan/dim/...` _palette_ in `style.rs` is reused; only the gate differs.

## Topic registry & source of truth

Bodies are a curated allowlist of existing in-repo docs, embedded verbatim via
`include_str!`. The `TOPICS` table **is** the curation boundary: dev-internal
specs (IR schema, `docs/dev/`) have no slug and can never leak into the
user-facing surface.

```rust
// rust/crates/cli/src/docs.rs
struct Topic {
    slug:    &'static str,
    aliases: &'static [&'static str],
    summary: &'static str,   // shown in the index and --json
    body:    &'static str,   // include_str! of a curated user-facing doc
}

// paths are four `../` to repo root, matching envelope.rs:32's
// include_str!("../../../../ir/VERSION")
const TOPICS: &[Topic] = &[
    Topic { slug: "agents", aliases: &["agent", "ai"],
        summary: "Orientation for agents: canonical workflow, idioms, when to ask the human",
        body: include_str!("../../../../AGENTS.md") },
    Topic { slug: "getting-started", aliases: &["intro", "start", "tutorial", "modeling", "model"],
        summary: "Write your first .camdl by example: compartments, transitions, rates, stratification",
        body: include_str!("../../../../docs/intro.md") },
    Topic { slug: "language", aliases: &["dsl", "spec", "syntax"],
        summary: "Full DSL reference: units & dimensions, parameter kinds, tables, forcings",
        body: include_str!("../../../../docs/camdl-language-spec.md") },
    Topic { slug: "inference", aliases: &["fit", "fitting", "mcmc"],
        summary: "Fitting: particle filter, IF2, PGAS+NUTS, profiles, diagnostics",
        body: include_str!("../../../../docs/inference.md") },
    Topic { slug: "features", aliases: &["catalogue"],
        summary: "Feature catalogue with the pomp comparison",
        body: include_str!("../../../../docs/user-features.md") },
    Topic { slug: "backends", aliases: &["runtimes"],
        summary: "Simulation backends: Gillespie, tau-leap, chain-binomial, ODE",
        body: include_str!("../../../../docs/runtimes.md") },
    Topic { slug: "data", aliases: &["observations", "obs"],
        summary: "Observation data format (time × dims tables)",
        body: include_str!("../../../../docs/camdl-data-spec.md") },
    Topic { slug: "debugging", aliases: &["debug", "eval", "trace"],
        summary: "Debugging via camdl eval and the substep tracer",
        body: include_str!("../../../../docs/debugging.md") },
];
```

Slugs mirror the camdl-book's native sections (`guide/getting-started`,
`language/`, `inference/`, `reference/features`), so an agent or human moving
between the book and the CLI sees one vocabulary. `include_str!` registers each
file as a build dependency: edit `docs/intro.md` and the cli rebuilds; the
embedded copy cannot go stale relative to the tree.

## Rendering & syntax highlighting (TTY path)

Two layers, both off the embedded source:

**Markdown structure → ANSI.** Recommend `pulldown-cmark` (a focused pull
parser) + a small writer that maps events to the `style.rs` palette: headings
bold/cyan, emphasis, lists, and — load-bearing for the error/diagnostics tables
in `agents` — table layout. (`termimad` renders boxed tables out of the box but
pulls in `crossterm` and friends; given the house preference for minimal deps,
start with `pulldown-cmark` + hand-rolled table alignment, and adopt `termimad`
only if boxed tables prove worth the weight.)

**`camdl` code fences → highlighted.** Reuse the existing grammar:

```rust
use tree_sitter_highlight::{Highlighter, HighlightConfiguration};

// highlights.scm defines 22 standard captures (@keyword, @type, @function,
// @variable.parameter, @attribute, @keyword.conditional, …) — verified at
// tree-sitter/queries/highlights.scm (201 lines)
static CAPTURES: &[&str] = &[/* the names from highlights.scm */];

let mut cfg = HighlightConfiguration::new(
    tree_sitter_camdl::language(),               // tree-sitter/bindings/rust exposes this
    "camdl",
    include_str!("../../../../tree-sitter/queries/highlights.scm"),
    "",                                          // injections (none)
    include_str!("../../../../tree-sitter/queries/locals.scm"),
).unwrap();
cfg.configure(CAPTURES);
// Highlighter emits HighlightStart(idx)/Source/HighlightEnd; wrap Source
// spans in style.rs ANSI keyed by idx.
```

Non-`camdl` fences (`toml` for fit.toml, `bash` for commands) render plain in v1
— no extra grammars. The win is the single-grammar property: `highlights.scm`
now feeds editors **and** the CLI (and, later, web docs) from one maintained
source. This is precisely why the `bat`/syntect route is wrong for us: it would
fork a second `.sublime-syntax` grammar.

**Paging.** When `Sink::Tty` and not `--no-pager`: pipe rendered output to
`$PAGER` (default `less -RF` — `-R` passes ANSI, `-F` skips paging if it fits
one screen). Plain `std::process::Command`; no pager crate needed.

## Discoverability — the `--help` doorbell

The subcommand is only as good as an agent's odds of finding it. `camdl --help`
is the one surface an agent reflexively probes. Add one line to the top-level
`after_help` (via the existing `colored_help!` / `colorize_after_help` path,
`style.rs:68`):

```
New to camdl?  Run `camdl docs` for guides (getting-started, inference, language…).
```

`camdl docs --help` lists the topic index. Topic slugs are registered as dynamic
shell-completion candidates so `camdl docs <TAB>` enumerates them.

## Examples — `camdl docs example`

Recovers the one real advantage the shallow-clone path had: **runnable** models.
`ocaml/golden/` holds 33 working `.camdl` files; a curated subset is embedded
and emitted to stdout.

```
camdl docs example                 # list embedded examples + one-line each
camdl docs example sir_basic       # emit the .camdl to stdout
camdl docs example sir_basic --params   # also emit its sir_basic.params.toml
```

An agent runs `camdl docs example seir_age > model.camdl` and has a correct,
compiling starting point — no guessing from pomp priors. CI invariant: every
embedded example must pass `camdl check` (reuse the existing golden gate), so a
shipped example is never broken.

## Search — `camdl docs search`

```
camdl docs search prior refinement
inference › Priors and precedence : 558
agents    › Idioms / anti-idioms  : 260
```

**Algorithm: linear scan, no index.** The corpus is ~8–10 docs / a few hundred
KB; an inverted index or BM25 would be premature optimization (a full scan is
sub-millisecond) and dead weight in the binary. The default match is
**case-insensitive, multi-term AND, line-oriented**: the query splits on
whitespace and a line matches if it contains _all_ terms, order-independent — so
`search prior refinement` hits a line carrying both words, where a plain
whole-query substring would miss it. `--regex` is opt-in (default stays literal
so a stray `.`/`(` doesn't surprise). The search space includes topic slugs,
aliases, and summaries, not just bodies, so `search pmmh` surfaces `inference`
even when the body phrases it differently.

**Section-aware and ranked.** While scanning, track the nearest preceding
markdown heading and emit `topic › Heading : line` — the cheap substitute for
the section anchors deferred from v1, since every hit names the section to read.
Results group by topic, topics ordered by hit count. Output honors the Sink
split: at a TTY, grouped with heading context and the matched span highlighted;
piped, plain `slug:lineno:text` (ripgrep-shaped, greppable).

## Mechanism & wiring

- **New module** `rust/crates/cli/src/docs.rs`: the `TOPICS` table, the `Sink`
  resolution, the renderer, the search, and example emission.
- **Enum site:** add `Docs(DocsArgs)` to `pub(crate) enum Command`
  (`rust/crates/cli/src/main.rs:93`) plus a dispatch arm. `DocsArgs` is a small
  clap struct: optional positional `topic`, the sub-verbs `search`/`example`,
  and the flags above.
- **New cli dependencies:** `tree-sitter`, `tree-sitter-highlight`,
  `tree-sitter-camdl` (`path = "../../../tree-sitter"`), `pulldown-cmark`. The
  grammar's `parser.c` compiles via the grammar crate's own `build.rs`; no
  change to `cli/build.rs` required.
- **Embedding precedent:** identical to `ir/src/envelope.rs:32` (`include_str!`
  of `ir/VERSION`) and `cli/src/landscape_html.rs:26` (`include_str!` of
  vendored Plotly + `pairplot.js`). Binary grows by the embedded docs (~tens of
  KB) plus the grammar tables — well within established tolerance
  (landscape_html already embeds a multi-MB JS bundle).

## Conformance with existing CLI policy

- **Color/TTY:** gate content on `stdout.is_terminal()`, honor `NO_COLOR` and
  `--color`, per `2026-05-30-cli-io-and-progress-ux.md`. Reuse the `style.rs`
  palette; do **not** reuse `style::enabled()` (stderr-gated) for content.
- **Error messages:** unknown topic/example errors list the valid set with a
  hint, per the repo's "error messages are a feature" principle.
- **Small surface:** four verbs, one mental model — "topics you can read or
  search, examples you can emit." Keeps "the grammar fits in a head."

## No stale commands in embedded docs (CI invariant)

Embedded docs are version-locked to the binary — that is the feature, and the
obligation. A stale command on a web page is a nuisance; a stale command in a
doc the binary _vouches for_ is a trap, because an agent runs it verbatim
precisely because `camdl docs` emitted it.

**No `camdl` command appears in an embedded topic or example unless it parses
against the current CLI.** Enforced as a CI gate: extract every line matching
`^\s*camdl` (and inline `` `camdl …` `` spans) from all embedded bodies and
examples and run each through `Cli::try_parse_from`. Any unknown subcommand or
unknown/renamed flag fails the build. This validates the command _surface_ — it
catches the exact staleness mode (a flag is renamed, the doc keeps the old name)
at build time, which, given version-locking, is the right place to catch it.
`try_parse` recognizes flags without touching the filesystem, so placeholder
paths like `model.camdl` are fine; it does **not** check that the command does
what the surrounding prose claims — that still needs human review. The same gate
runs over any content promoted from the book before it lands.

## Testing

- `camdl docs --json` returns the stable topic set; snapshot it.
- Every topic body is non-empty and round-trips through the markdown parser.
- Every embedded example passes `camdl check` (CI gate, reuses golden infra).
- **No-stale-command gate** (above): every `camdl …` in an embedded body or
  example parses against the live CLI, or the build fails.
- **Regression for the `show` bug:** `camdl docs inference | cat` emits **zero**
  ESC bytes (`assert no \x1b`). This pins the stdout-gate so content coloring
  can never leak into a pipe.
- Unknown topic exits nonzero and names the valid slugs.
- Alias resolution: `camdl docs fit` == `camdl docs inference`.

## Effort & phasing

The whole feature is one ship. Two internal layers, by where the work sits:

- **Core** (registry + embed + raw output + index + `--json` + `--all` +
  search + example + `--help` doorbell + completion): the bulk, all
  straightforward string/IO plumbing, no new heavy deps beyond the markdown
  parser. This alone closes the discoverability gap.
- **Render layer** (markdown→ANSI + tree-sitter highlighting + pager): the part
  that adds the grammar + highlight deps and the `parser.c` compile. Cheap
  _because the grammar and highlight query already exist_ — the hard part was
  done for the editor tooling.

Recommend landing both together so the human experience is right on first
release, but the core is independently shippable if we want a checkpoint.

## Future refinements

- **Factor `AGENTS.md` into orientation + addressable reference.** Keep
  `AGENTS.md` as the single orientation briefing (mental model, canonical
  workflow, when-to-ask) and the cross-tool-standard entry point; move its
  _reference_ slabs — the error→fix table, the diagnostics table, the fit.toml
  schema — into single-sourced docs that `camdl docs errors` / `diagnostics` /
  `fit-toml` serve and that `AGENTS.md` links to. Read-once and lookup are
  different access patterns; today they are crammed in one file. Only worth
  doing once `camdl docs` exists to serve the slices.
- **Distill the book's fitting workflows into in-repo topics.** `AGENTS.md`
  _describes_ the workflow abstractly; nothing in-repo _shows_ a full
  prior-refinement loop on real data. The camdl-book has this material and
  `docs/` does not. From a survey of the book, the highest-value promotions are
  a **fitting-workflow + diagnostics** topic (the scout/refine/validate
  narrative plus the Healthy/Warning/Action diagnostic tables, from
  `inference/diagnostics.qmd` + `guide/fitting/`) and a **prior-refinement /
  identifiability** topic distilled from the WA-State cryptic-introduction case
  study (`guide/fitting/seed-timing.qmd`): synthetic recovery → bound-pinning
  diagnosis → weakly-informative prior refinement → 2D ridge profile → external
  validation against an independent genomic estimate. These become new
  `camdl docs` topics, but the distillation (book Quarto + Python → clean
  markdown + camdl commands) is its own effort, separate from this subcommand.
  Verify every promoted command against the current CLI before it lands — some
  book snippets reference not-yet-shipped flags.
- **Section anchors** (`camdl docs inference#diagnostics`) if `search` proves
  insufficient.
- **More fence grammars** (`toml`, `bash`) if plain rendering of those blocks
  reads poorly.
- **Man-page / completion generation** from the same `TOPICS` source.
