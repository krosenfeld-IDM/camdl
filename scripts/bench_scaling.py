#!/usr/bin/env python3
"""Macro scaling sweep for the camdl FOI blowup study.

For each (P, A, coupling, grad) scale point this driver:
  1. generates a toy .camdl (scripts/gen_scaling_models.gen_camdl)
  2. compiles it  (camdl compile --no-dim-check)  → compile_s, ir_bytes
  3. simulates from the IR under /usr/bin/time -l  → sim_s, peak_rss_mb
and appends a row to a TSV. Measurement only — no plotting deps (stdlib).
Plot with scripts/plot_scaling.py afterwards.

Stdlib only. Run directly (python3 scripts/bench_scaling.py ...); the matched
camdlc is located via the CAMDLC env var (set by the Makefile target).

The forward `simulate` cost depends on (P, A, coupling) but NOT on `grad`
(only the `rate` tree is evaluated, never `rate_grad`); IR size / compile /
parse / RSS depend on all four. The grid below exploits that separation.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen_scaling_models import gen_camdl  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
CAMDL = REPO / "rust" / "target" / "release" / "camdl"
WORK = Path("/tmp/scaling_sweep")

_MAXRSS = re.compile(r"^\s*(\d+)\s+maximum resident set size", re.M)
_REAL = re.compile(r"^\s*([\d.]+)\s+real", re.M)


def _grid() -> list[dict]:
    """The scale points. Three slices, each isolating one effect."""
    pts: list[dict] = []
    # Slice 1 — P-exponent (cheap: A=1, grad=minimal). on vs off shows O(P^2) vs O(P).
    # coupling=on caps at P=44: at P>=~50 the flat-inlined spatial sum nests a
    # BinOp::Add chain deep enough to trip serde_json's recursion limit (128) at
    # IR-parse time — a hard cliff, probed separately in the findings note.
    for P in (2, 4, 8, 16, 32, 44):
        pts.append(dict(P=P, A=1, coupling="on", grad="minimal", slice="slope"))
    for P in (2, 4, 8, 16, 32, 44, 64, 88):
        pts.append(dict(P=P, A=1, coupling="off", grad="minimal", slice="slope"))
    # Slice 2 — realism / anchor (A=21, coupling on). Includes the real-model point
    # P=44,A=21,on,full (~2.6 GB IR / ~8 GB RSS). minimal vs full isolates rate_grad.
    for grad in ("minimal", "full"):
        for P in (4, 8, 16, 32, 44):
            pts.append(dict(P=P, A=21, coupling="on", grad=grad, slice="realism"))
    # Slice 3 — gradient multiplier at moderate A (A=7), coupling on.
    for grad in ("minimal", "full"):
        for P in (4, 8, 16, 32):
            pts.append(dict(P=P, A=7, coupling="on", grad=grad, slice="grad"))
    return pts


def _timed(cmd: list[str], env: dict) -> tuple[float, int, int]:
    """Run `cmd` under /usr/bin/time -l. Returns (real_s, maxrss_bytes, returncode)."""
    full = ["/usr/bin/time", "-l", *cmd]
    proc = subprocess.run(full, env=env, capture_output=True, text=True)
    err = proc.stderr
    real = float(m.group(1)) if (m := _REAL.search(err)) else float("nan")
    rss = int(m.group(1)) if (m := _MAXRSS.search(err)) else -1
    return real, rss, proc.returncode


def run(out_tsv: Path, only_slice: str | None) -> None:
    if not CAMDL.exists():
        sys.exit(f"camdl binary not found at {CAMDL} — run `make build-rust` first")
    camdlc = os.environ.get("CAMDLC")
    if not camdlc or not Path(camdlc).exists():
        sys.exit("set CAMDLC to the matched camdlc.exe (see Makefile bench-scaling target)")
    WORK.mkdir(parents=True, exist_ok=True)
    out_tsv.parent.mkdir(parents=True, exist_ok=True)

    env = {**os.environ, "TMPDIR": "/tmp", "CAMDL_SKIP_VERSION_CHECK": "1"}
    cols = ["slice", "P", "A", "coupling", "grad", "n_compartments", "n_transitions",
            "ir_bytes", "compile_s", "sim_s", "peak_rss_mb"]
    rows: list[list] = []
    pts = [p for p in _grid() if only_slice is None or p["slice"] == only_slice]
    print(f"sweep: {len(pts)} scale points → {out_tsv}", file=sys.stderr)

    for i, p in enumerate(pts, 1):
        P, A, coup, grad = p["P"], p["A"], p["coupling"], p["grad"]
        tag = f"P{P}_A{A}_{coup}_{grad}"
        camdl_f = WORK / f"{tag}.camdl"
        ir_f = WORK / f"{tag}.ir.json"
        camdl_f.write_text(gen_camdl(P, A, coup, grad))

        print(f"[{i}/{len(pts)}] {p['slice']:8s} {tag} ... ", end="", file=sys.stderr, flush=True)
        compile_s, _, rc = _timed(
            [str(CAMDL), "compile", str(camdl_f), "--no-dim-check", "-o", str(ir_f)], env)
        if rc != 0 or not ir_f.exists():
            print("COMPILE FAIL", file=sys.stderr)
            continue
        ir_bytes = ir_f.stat().st_size

        sim_cmd = [str(CAMDL), "simulate", str(ir_f),
                   "--backend", "chain_binomial", "-o", str(WORK / "traj.tsv")]
        if grad == "full":
            sim_cmd += ["--scenario", "baseline"]
        sim_s, rss, rc = _timed(sim_cmd, env)
        rss_mb = rss / 1e6 if rss > 0 else float("nan")
        if rc != 0:
            print("SIM FAIL", file=sys.stderr)
            ir_f.unlink(missing_ok=True)
            continue

        n_comp = 4 * P * A
        n_tr = 3 * P * A  # infection + progression + recovery
        rows.append([p["slice"], P, A, coup, grad, n_comp, n_tr,
                     ir_bytes, f"{compile_s:.3f}", f"{sim_s:.3f}", f"{rss_mb:.1f}"])
        print(f"ir={ir_bytes/1e6:8.2f}MB compile={compile_s:6.2f}s "
              f"sim={sim_s:6.2f}s rss={rss_mb/1000:6.2f}GB", file=sys.stderr)

        # Write incrementally so a mid-sweep crash still yields partial data.
        with out_tsv.open("w") as f:
            f.write("\t".join(cols) + "\n")
            for r in rows:
                f.write("\t".join(str(x) for x in r) + "\n")
        ir_f.unlink(missing_ok=True)  # bound transient disk (IR can be GBs)

    print(f"done: {len(rows)} rows → {out_tsv}", file=sys.stderr)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("-o", "--out", type=Path,
                    default=REPO / "docs/dev/notes/assets/scaling/scaling.tsv")
    ap.add_argument("--slice", choices=("slope", "realism", "grad"), default=None,
                    help="run only one slice (default: all)")
    args = ap.parse_args()
    run(args.out, args.slice)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
