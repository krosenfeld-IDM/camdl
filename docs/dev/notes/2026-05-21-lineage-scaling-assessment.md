# Scaling assessment: event log → line list → tree

Date: 2026-05-21
Project: camdl (lineage layer)
Tags: lineage, scaling, streaming, newick, phylodynamics

## Context / question

The three-layer lineage workflow (event log → line list → tree) — how far does
it scale toward millions of individuals, where are the ceilings, and what is the
state of the art for the part that doesn't scale (the Newick tree endpoint)?

This note assesses the current code stage-by-stage, then surveys SOTA
large-tree representations, then recommends a direction.

## Stage-by-stage assessment (code as on `main`, 2026-05-21)

### Stage 0 — simulate → event log (write)

`EventRecorder` accumulates the whole run into `EventLog.events:
Vec<EventRecord>` in RAM, written at the end. `event_log_io::write_parquet`
collects every column into Vecs and emits **one** `RecordBatch` = **one Parquet
row group**.

- **Memory:** O(total recorded events). An `EventRecord` is `time` (f64),
  `transition`, `multiplicity` (u64), `batched` (bool), `step` (u64), and
  `lineage_weights: Option<Vec<f64>>` (length = #infector pools at a `#[lineage]`
  event; `None` elsewhere). Roughly 50–80 B + weights per event. On batched
  backends the log is *compressed* (one row per substep firing with a
  `multiplicity`), so it is smaller than the per-individual line list.
- **Ceiling:** comfortable to a few million events (~100s of MB); the single row
  group means the file can't be read incrementally either (see Stage 1).
- **Metadata is streaming-friendly already:** `initial_pools` + the route table
  go in Parquet **file-level key-value metadata** (footer) / TSV **header
  comment lines** — the event rows are pure payload. Nothing blocks streaming.

### Stage 1 — event log → line list (`realize`)

- **Read side:** `read_parquet` accumulates every row into one
  `Vec<EventRecord>`; `read_tsv` does `read_to_string` (whole file as a String)
  first. The single row group forces full materialization regardless.
- **Identity state (the surprise):** `IdentityState { pools: HashMap<(DemeId,
  CompartmentId), Vec<IndividualId>>, next: u64 }`. A progression `I→R` removes
  from the `I` pool and **pushes to the `R` pool**. `R` is absorbing — nothing
  removes from it — so **the `R` pool grows monotonically to O(cumulative
  infections) = O(N)**, not O(currently-alive). For an SIR over N people,
  `realize` holds ~N IndividualIds in the R pool. The R pool is *write-only*
  (R is never an infector/source), so this is pure overhead — optimizable.
- **Write side:** the line-list writer (`writer.rs`) **streams** — append one
  row, flush in `BATCH_ROWS = 8192` Parquet row groups, "we never buffer the
  whole log." This is the *largest* artifact (expanded one-row-per-individual-
  edge) and it is already disk-bound, not RAM-bound.
- **Ceiling:** `realize` resident set ≈ full event log (read) + O(N) identity
  pools (R). Two independent O(N) terms, both removable.

### Stage 2 — line list → tree (the hard one, as predicted)

`tree.rs`:

- `TransmissionForest::from_entries(&[LineListEntry])` takes the **whole line
  list as a slice** (already fully in RAM) and builds `HashMap<IndividualId,
  Node>` over **all** individuals, each `Node` carrying a `children: Vec`. Two
  O(N) structures (the slice + the map).
- **Sampling happens *after* the full build:** `prune_to` operates on the
  already-built full forest. Subsampling to 1,000 tips does **not** reduce the
  build-time memory — you pay O(N) to then throw most of it away.
- **Recursion on tree depth:** `build_pruned`, `write_newick`, `tip_count`,
  `sackin_at` are all recursive. Recursion depth = longest transmission chain.
  For a typical epidemic that is modest (≈ epidemic duration / generation
  interval, tens–low hundreds of generations), but it is **unbounded in
  principle** — a long endemic run or a deeply chained process can overflow the
  default 8 MB stack. A robustness ceiling (crash), not just a memory one.
- **Newick output is a monolithic in-RAM `String`** built with a per-node
  `format!` (allocation churn), size O(total tips). Millions of tips → a
  multi-GB string, and nothing downstream ingests a 10M-tip Newick happily.

**Verdict:** Stages 0–1 are streamable with bounded work; the tree is the hard
ceiling, exactly as expected. The three tree problems are independent: O(N)
build, depth-recursion crash risk, and the Newick string.

## SOTA: how pandemic-scale phylogenetics solved the large-tree problem

The SARS-CoV-2 "millions of genomes" effort hit precisely this wall and the
field's answers are directly relevant.

1. **Newick does not scale, and the cost is quantified.** Newick is a recursive
   parenthetical *text* string: O(n) but with bad constants, deep nesting
   (parser/stack), no random access, no streaming. Taxonium reports **~80 s just
   to deserialize a 5.4 M-tip Newick** into memory (Sanderson 2022). Treat
   Newick as a small-/subsampled-tree *export*, never the canonical artifact at
   scale.

2. **UShER MAT (mutation-annotated tree), protobuf** — the gold standard
   (Turakhia et al. 2021, *Nat. Genet.* 53:809; McBroome et al. 2021, *MBE*
   38(12):5819). A compact **binary** tree with per-branch annotations:
   834,521 sequences = **65 MB (14 MB gzipped)**, ≈300× smaller than the MSA.
   `matUtils` converts MAT↔Newick on demand. The lesson: a structured,
   annotation-bearing **binary** tree, generated once and converted to text only
   when a small slice is needed.

3. **Taxonium JSONL** (Sanderson 2022, *eLife* 11:e82392) — a separate format
   for *visualization* of tens of millions of nodes (pre-computed layout, WebGL,
   client-side streaming). Lesson: **separate the storage representation from
   the visualization representation**; don't force one format to do both.

4. **Phylo2Vec** (Penn et al. 2024, *Syst. Biol.* 74(2):250; library 2025) — a
   bijective **integer-vector** encoding of binary-tree *topology* (length n−1):
   more compact than Newick, O(1) topological-identity check, good for ML and
   tree-space moves. Most relevant for topology manipulation, less for
   annotation-rich transmission trees, but it is the "compact integer encoding"
   reference point.

5. **The columnar edge-list / parent-array representation — and the key
   realization: camdl already has it.** A tree is fully specified by an edge
   list `(parent_id, child_id, branch_length)` (or a parent array `parent[i]`).
   That is columnar, random-accessible, streamable, and Arrow/Parquet-native.
   **camdl's line list (`individual`, `parent_id`, `time`, `deme`, …) is exactly
   a columnar edge list of the transmission forest.** So the scalable tree
   representation is *already the on-disk artifact*; Newick is just one
   un-scalable serialization of it.

## Interpretation / recommendation

**Reframe:** the line list *is* the scalable tree (a columnar edge list in
Parquet). The pipeline already stores the genealogy in the SOTA-aligned form.
"The tree" as Newick is a lossy, un-streamable *export* — it should be a
small/subsampled convenience, not the canonical object.

Concrete directions, in impact order:

1. **Stream Stages 0–1 (the easy, high-value wins).** Chunk the event-log
   Parquet writer into row groups (mirror `writer.rs`), stream the reader
   batch-by-batch into `realize`, and stop pushing to write-only absorbing
   destination pools (the R pool). This drops both simulate-side and realize-side
   resident memory to ~O(alive), with no format change — Parquet streams natively
   (the line-list writer already proves it). Maintain the time-sort invariant
   (free with streaming; add a debug assertion that event times are
   non-decreasing).

2. **Sample *before* building the tree.** A streaming pass over the line list to
   (a) choose sampled tips and (b) build a cheap parent-pointer map
   (`id → parent`), then materialize only the induced subtree. Bounds tree memory
   to O(sampled + their ancestors) instead of O(N) — the single most impactful
   tree change, and it makes "subsample to 5k tips from a 10M-person epidemic"
   actually cheap.

3. **Make the tree traversals iterative** (explicit stack) for prune / Newick /
   stats. Removes the depth-recursion stack-overflow ceiling independent of
   scale — a robustness fix.

4. **Stream Newick generation** (write incrementally to a `Write`r via an
   iterative traversal) rather than building one giant `String`; and treat
   Newick as the *subsampled* export only.

5. **Don't invent a tree format.** For a large annotated single-file tree, the
   edge-list Parquet (the line list) + a small roots/sampling sidecar already
   serves; a MAT-style protobuf is the precedent if a self-contained binary tree
   artifact is ever wanted. Newick↔edge-list conversion on demand mirrors
   `matUtils`.

**Bottom line for "millions of people":** Stages 0–1 reach disk-bound with the
streaming fixes (item 1), and trees become tractable by subsampling-before-build
(item 2) — which is also what every phylodynamic method actually does. The thing
to *stop* doing at scale is materializing a full Newick; the columnar line list
is the canonical, scalable genealogy and is already what's on disk.

## References

- Turakhia, Y. et al. (2021). *Ultrafast Sample placement on Existing tRees
  (UShER) enables real-time phylogenetics for the SARS-CoV-2 pandemic.* Nat.
  Genet. 53:809–816.
- McBroome, J. et al. (2021). *A daily-updated database and tools for
  comprehensive SARS-CoV-2 mutation-annotated trees.* Mol. Biol. Evol.
  38(12):5819–5824.
- Sanderson, T. (2022). *Taxonium, a web-based tool for exploring large
  phylogenetic trees.* eLife 11:e82392.
- Penn, M. J. et al. (2024). *Phylo2Vec: a vector representation for binary
  trees.* Syst. Biol. 74(2):250–266. (+ phylo2vec library, arXiv:2506.19490.)
