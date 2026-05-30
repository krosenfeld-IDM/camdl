SHELL := bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := build

# ── Paths ─────────────────────────────────────────────────────────────────────

CAMDLC  := ocaml/_build/default/bin/camdlc.exe
CAMDL   := rust/target/release/camdl
INSTALL_DIR ?= $(HOME)/.local/bin

OCAML_GOLDENS := $(wildcard ocaml/golden/*.camdl)

# ── Build ─────────────────────────────────────────────────────────────────────

.PHONY: build build-ocaml build-rust

build: build-ocaml build-rust

# gh#audit-C8 follow-up. ir/VERSION is the canonical IR schema version
# (Rust reads it via include_str! at compile time). OCaml's dune project
# root is `ocaml/`, which puts ir/VERSION outside dune's source tree —
# so we generate a tiny .ml constant module from the file *before* dune
# runs, guaranteeing both languages bake the same value at build time.
# The generated file is .gitignore'd; bumping ir/VERSION + `make build`
# re-emits it.
OCAML_IR_VERSION_GEN := ocaml/lib/ir/ir_version_generated.ml

$(OCAML_IR_VERSION_GEN): ir/VERSION
	@printf '(* GENERATED from ir/VERSION by Makefile — do not edit. *)\nlet value = "%s"\n' \
	    "$$(tr -d '[:space:]' < ir/VERSION)" > $@

build-ocaml: $(OCAML_IR_VERSION_GEN)
	cd ocaml && dune build

build-rust:
	cd rust && cargo build --release --workspace --bins

# ── Install ───────────────────────────────────────────────────────────────────

.PHONY: install uninstall

# Git hash embedded in both binaries for version-skew detection.
GIT_HASH := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

install: build
	@mkdir -p $(INSTALL_DIR)
	@# camdlc: dune uses .exe on all platforms; install without the suffix.
	@# Also install as camdlc-<hash> so camdl can confirm an exact version
	@# match via a filesystem stat (no subprocess needed).
	install -m 755 $(CAMDLC) $(INSTALL_DIR)/camdlc
	install -m 755 $(CAMDLC) $(INSTALL_DIR)/camdlc-$(GIT_HASH)
	install -m 755 $(CAMDL)  $(INSTALL_DIR)/camdl
	@echo "Installed to $(INSTALL_DIR)  [camdlc-$(GIT_HASH)]"
	@echo "Make sure $(INSTALL_DIR) is on your PATH."
	@# Postflight: detect when another `camdl` (typically a leftover
	@# `cargo install --path crates/cli` in ~/.cargo/bin/) wins on PATH
	@# ahead of the binary we just wrote. Without this check the user
	@# only finds out at first invocation, and the runtime error tells
	@# them to "run make install" — which they just did. Catch it now.
	@expected=$(INSTALL_DIR)/camdl; \
	first=$$(command -v camdl 2>/dev/null || true); \
	if [ -n "$$first" ] && [ "$$first" != "$$expected" ]; then \
	  echo ""; \
	  echo "warning: another \`camdl\` is shadowing this install on your PATH."; \
	  echo "  Resolves first on PATH: $$first"; \
	  echo "  Just installed:         $$expected"; \
	  echo "  Fix: \`rm $$first\`, or put $(INSTALL_DIR) ahead of $${first%/*} on your PATH."; \
	fi

uninstall:
	rm -f $(INSTALL_DIR)/camdlc $(INSTALL_DIR)/camdl
	rm -f $(INSTALL_DIR)/camdlc-$(GIT_HASH)
	@echo "Removed from $(INSTALL_DIR)"

# ── Test ──────────────────────────────────────────────────────────────────────

.PHONY: test test-ocaml test-rust test-integration

test: test-ocaml test-rust test-integration

test-ocaml:
	cd ocaml && dune runtest

test-rust:
	cd rust && cargo test --workspace

test-integration: build
	CAMDLC="$(CAMDLC)" CAMDL="$(CAMDL)" bash tests/test_ocaml_to_rust.sh

# ── Golden file management ────────────────────────────────────────────────────

.PHONY: update-golden update-ocaml-golden

# Recompile all DSL fixtures → ocaml/golden/*.ir.json
update-ocaml-golden: build-ocaml
	@echo "Recompiling OCaml golden files..."
	@for src in $(OCAML_GOLDENS); do \
		out="$${src%.camdl}.ir.json"; \
		echo "  $$src → $$out"; \
		$(CAMDLC) "$$src" > "$$out"; \
	done

update-golden: update-ocaml-golden

# ── Quick simulation helpers ──────────────────────────────────────────────────

.PHONY: sim

# Usage: make sim MODEL=ir/golden/sir_basic.ir.json ARGS="--set beta=0.3 ..."
sim: build-rust
	$(CAMDL) simulate $(MODEL) $(ARGS)

# ── Benchmarks & profiling (FOI scaling study) ────────────────────────────────
#
# See docs/dev/notes/2026-05-29-foi-scaling-bench.md. The toy model generator
# is scripts/gen_scaling_models.py; macro sweep scripts/bench_scaling.py.

.PHONY: bench-scaling bench-micro bench-micro-fixtures flamegraph-real flamegraph-bench profile-pmmh

CAMDLC_ABS := $(abspath $(CAMDLC))
GEN        := scripts/gen_scaling_models.py
FX         := rust/crates/sim/benches/fixtures/scaling
PROFILE_CAMDL := rust/target/profiling/camdl

# (P,A,coupling) grid for the micro-bench fixtures — matches GRID in scaling.rs.
MICRO_GRID := 4/1/on 8/1/on 16/1/on 32/1/on 4/1/off 8/1/off 16/1/off 32/1/off \
              8/7/on 16/7/on 32/7/on 8/7/off 16/7/off 32/7/off

# Macro sweep: full compile→simulate pipeline across scales → TSV + plot.
bench-scaling: build
	CAMDLC="$(CAMDLC_ABS)" python3 scripts/bench_scaling.py
	uv run --with matplotlib --with numpy scripts/plot_scaling.py

# Generate the (gitignored) IR fixtures the micro-bench loads.
bench-micro-fixtures: build
	@mkdir -p $(FX)
	@for spec in $(MICRO_GRID); do \
	  P=$${spec%%/*}; rest=$${spec#*/}; A=$${rest%%/*}; C=$${rest##*/}; \
	  out=$(FX)/P$${P}_A$${A}_$${C}_minimal.ir.json; \
	  python3 $(GEN) -P $$P -A $$A --coupling $$C --grad minimal -o /tmp/_micro.camdl 2>/dev/null; \
	  CAMDL_SKIP_VERSION_CHECK=1 CAMDLC="$(CAMDLC_ABS)" $(CAMDL) compile /tmp/_micro.camdl --no-dim-check -o $$out >/dev/null; \
	done
	@echo "fixtures → $(FX)"

# Per-step eval / load micro-benchmarks (criterion): the `scaling` bench.
bench-micro: bench-micro-fixtures
	cd rust && cargo bench -p sim --bench scaling

# Flamegraph the real-model regime: generate the anchor (P=44,A=21,coupling=on,
# grad=full ≈ the Kano model), then profile `simulate`. Produces a static SVG
# (macOS `sample` → inferno; no sudo) that serves cleanly over HTTP, plus a
# samply profile for interactive exploration. Point at a different IR (e.g. the
# real Kano model) to profile that instead.
# Prereqs: `cargo install inferno samply`.
FG_SVG := docs/dev/notes/assets/scaling/flamegraph_real.svg
flamegraph-real: build-ocaml
	cd rust && cargo build --profile profiling -p cli --bin camdl
	python3 $(GEN) -P 44 -A 21 --coupling on --grad full -o /tmp/fg_anchor.camdl
	CAMDL_SKIP_VERSION_CHECK=1 CAMDLC="$(CAMDLC_ABS)" $(PROFILE_CAMDL) \
	  compile /tmp/fg_anchor.camdl --no-dim-check -o /tmp/fg_anchor.ir.json
	@echo "sampling simulate (~12s)..."
	@TMPDIR=/tmp CAMDL_SKIP_VERSION_CHECK=1 CAMDLC="$(CAMDLC_ABS)" $(PROFILE_CAMDL) \
	   simulate /tmp/fg_anchor.ir.json --backend chain_binomial --scenario baseline \
	   -o /tmp/fg_traj.tsv & \
	 PID=$$!; sample $$PID 12 -file /tmp/camdl_sample.txt >/dev/null 2>&1; wait $$PID
	inferno-collapse-sample /tmp/camdl_sample.txt | \
	  inferno-flamegraph --title "camdl simulate anchor (P=44,A=21,on,full)" > $(FG_SVG)
	@echo "wrote $(FG_SVG)  (also: samply record -- $(PROFILE_CAMDL) simulate … for interactive)"

# Flamegraph the per-step hot path via the scaling bench binary.
flamegraph-bench: bench-micro-fixtures
	cd rust && cargo build --profile profiling -p sim --bench scaling
	@echo "run: samply record -- rust/target/profiling/deps/scaling-* --bench --profile-time 10 eval_propensities"

# Flamegraph PMMH inference steps. `--observe` makes the generated spatial model
# fittable (a weekly_cases stream over prevalence(I)); we synthesize data, then
# sample a PMMH run → static inferno SVG. PMMH is particle-filter-based (uses
# `rate` only, no rate_grad path); `--grad full` only supplies free FOI params
# for PMMH to estimate. Memory-safe at moderate P — small IR, PF state is just
# N_particles × compartments — so unlike `flamegraph-real` (P=44,A=21 full grad,
# the ~15 GB OOM anchor) this stays small. Tune: PMMH_P/PMMH_A/PMMH_STEPS/PMMH_PARTICLES.
PMMH_P ?= 16
PMMH_A ?= 7
PMMH_STEPS ?= 100
PMMH_PARTICLES ?= 200
FG_PMMH_SVG := docs/dev/notes/assets/scaling/flamegraph_pmmh.svg
profile-pmmh: build-ocaml
	cd rust && cargo build --profile profiling -p cli --bin camdl
	python3 $(GEN) -P $(PMMH_P) -A $(PMMH_A) --coupling on --grad full --observe -o /tmp/pmmh_anchor.camdl
	CAMDL_SKIP_VERSION_CHECK=1 CAMDLC="$(CAMDLC_ABS)" $(PROFILE_CAMDL) \
	  compile /tmp/pmmh_anchor.camdl --no-dim-check -o /tmp/pmmh_anchor.ir.json
	CAMDL_SKIP_VERSION_CHECK=1 $(PROFILE_CAMDL) simulate /tmp/pmmh_anchor.ir.json \
	  --backend chain_binomial --dt 1 --seed 42 --scenario baseline --obs-dir /tmp/pmmh_obs >/dev/null
	@echo "sampling PMMH (~15s); P=$(PMMH_P) A=$(PMMH_A) particles=$(PMMH_PARTICLES) steps=$(PMMH_STEPS)..."
	@TMPDIR=/tmp CAMDL_OUTPUT_DIR=/tmp/pmmh_prof_out CAMDL_SKIP_VERSION_CHECK=1 $(PROFILE_CAMDL) \
	   profile /tmp/pmmh_anchor.ir.json --scenario baseline \
	   --data /tmp/pmmh_obs/weekly_cases.tsv --obs weekly_cases --flow infection \
	   --sweep 'R0=lin(14,16,2)' --particles $(PMMH_PARTICLES) \
	   --algorithm pmmh --pmmh-steps $(PMMH_STEPS) --pmmh-particles $(PMMH_PARTICLES) --pmmh-rho 0.99 \
	   --starts 1 --rw-sd auto --fixed sigma=0.125 --fixed kappa=0.05 --fixed amplitude=0.25 --fixed iota=1e-7 \
	   --output /tmp/pmmh_prof.tsv --seed 1 >/tmp/pmmh_prof.log 2>&1 & \
	 PID=$$!; sample $$PID 15 -file /tmp/pmmh_sample.txt >/dev/null 2>&1; wait $$PID
	inferno-collapse-sample /tmp/pmmh_sample.txt | \
	  inferno-flamegraph --title "camdl PMMH step (P=$(PMMH_P),A=$(PMMH_A),coupling on)" > $(FG_PMMH_SVG)
	@echo "wrote $(FG_PMMH_SVG)"

# ── Tree-sitter / Neovim ──────────────────────────────────────────────────────

TS_DIR      := tree-sitter
NVIM_PARSER := $(HOME)/.local/share/nvim/lazy/nvim-treesitter/parser/camdl.so
NVIM_QUERIES := $(HOME)/.config/nvim/after/queries/camdl

.PHONY: install-nvim-ts

# Compile the camdl tree-sitter parser and install it into Neovim.
# Requires: a C compiler on PATH.
install-nvim-ts:
	@echo "Compiling tree-sitter parser..."
	cc -shared -fPIC -o $(TS_DIR)/camdl.so -I $(TS_DIR)/src $(TS_DIR)/src/parser.c
	install -m 644 $(TS_DIR)/camdl.so $(NVIM_PARSER)
	@echo "Installing queries..."
	@mkdir -p $(NVIM_QUERIES)
	install -m 644 $(TS_DIR)/queries/highlights.scm $(NVIM_QUERIES)/highlights.scm
	install -m 644 $(TS_DIR)/queries/locals.scm     $(NVIM_QUERIES)/locals.scm
	@echo "Done. Restart Neovim and open a .camdl file."

# ── Housekeeping ──────────────────────────────────────────────────────────────

.PHONY: clean

clean:
	cd ocaml && dune clean
	cd rust && cargo clean
