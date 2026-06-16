#!/usr/bin/env bash
# Regenerate the cached ODE incidence-oracle references (gh#166 Phase B).
#
# CI never runs this — it reads the committed ref/*.tsv fixtures. Run it only to
# re-cut the references after changing a model or its baked params (which must be
# kept in lock-step across models/<m>.camdl --set, scipy_oracle.py, and
# desolve_oracle.R). Requires: uv (for scipy) and Rscript + deSolve.
set -euo pipefail
cd "$(dirname "$0")"

echo "── scipy (RK45 + LSODA) ─────────────────────────────────────────"
uv run --quiet --with scipy --with numpy python scipy_oracle.py

echo "── deSolve (lsoda) ──────────────────────────────────────────────"
command -v Rscript >/dev/null || { echo "Rscript not found; install R + deSolve" >&2; exit 1; }
Rscript desolve_oracle.R

echo "── done; references in ../ref/ ──────────────────────────────────"
ls -1 ../ref/
