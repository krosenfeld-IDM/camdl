#!/usr/bin/env python3
"""Compiler-only timing sweep for camdlc (the OCaml frontend).

This is the *compile-side* analogue of scripts/bench_scaling.py. Where that
driver times the full `camdl compile` + `simulate` pipeline through the Rust
CLI, this one isolates the OCaml compiler: it invokes `camdlc.exe` directly on
a ladder of models and records wall time + peak RSS + emitted IR size. The Rust
runtime is never touched, so the numbers attribute cleanly to the frontend
(parse -> expand -> validate -> dimcheck -> autodiff -> serialize).

It reuses scripts/gen_scaling_models.gen_camdl for the synthetic ladder (the
same toy SEIR+spatial generator the runtime scaling study uses) so compile and
runtime curves are drawn from the *same* model family. Real models (e.g. the
Kano LGA SEIRV) are passed via --real and timed alongside.

Stdlib only. The matched camdlc is located via the CAMDLC env var (set by the
Makefile `bench-compile` target) or --camdlc; default is the in-tree build at
ocaml/_build/default/bin/camdlc.exe.

Usage:
    CAMDLC=ocaml/_build/default/bin/camdlc.exe python3 scripts/bench_compile.py \
        --out docs/dev/notes/assets/compile/compile_baseline.tsv \
        --reps 3 --real /path/to/kano_lga_seirv.camdl

Output: a TSV (machine-readable, written incrementally) and a markdown table
printed to stdout (paste into the baseline note). Dim-check is ON by default —
that is the cost a user actually pays; pass --no-dim-check to isolate it.
"""
from __future__ import annotations

import argparse
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen_scaling_models import gen_camdl  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
DEFAULT_CAMDLC = REPO / "ocaml" / "_build" / "default" / "bin" / "camdlc.exe"
WORK = Path("/tmp/compile_sweep")

_MAXRSS = re.compile(r"^\s*(\d+)\s+maximum resident set size", re.M)
_REAL = re.compile(r"^\s*([\d.]+)\s+real", re.M)

# Per-pass line emitted by ocaml/lib/ir/passtime.ml under CAMDL_TIME_PASSES.
PASSES = ("parse", "expand", "validate", "dimcheck", "autodiff", "serialize")
_PASS_LINE = re.compile(r"^\s+(\w+)\s+([\d.]+)\s+s\s", re.M)


def synthetic_grid() -> list[dict]:
    """The synthetic scale points. Three slices, each isolating one axis.

    All points are post-Fix-B safe (the deep-inlined O(P^2.A^2) IR is gone);
    the heaviest synthetic point (P=44,A=21,on,full) compiles in ~3 GB RSS.
    The real Kano SEIRV model (5 compartments) is heavier and timed via --real.
    """
    pts: list[dict] = []
    # Slice P — patch sweep at the realistic Kano shape (A=21, coupling on,
    # full grad). Draws the time-vs-patches curve where the spatial FOI lives.
    for P in (2, 4, 8, 16, 32, 44):
        pts.append(dict(P=P, A=21, coupling="on", grad="full", slice="patch"))
    # Slice A — age sweep at fixed patches (P=16, coupling on, full grad). The
    # P16_A7_on_full point doubles as the coupling-"on" half of the axis slice.
    for A in (1, 7, 14, 21):
        pts.append(dict(P=16, A=A, coupling="on", grad="full", slice="age"))
    # Slice axis — coupling-off control at the P16_A7 anchor. Compared against
    # the age slice's P16_A7_on_full, this isolates the O(P^2) spatial-coupling
    # cost. (grad=minimal is intentionally omitted: the toy generator bakes the
    # FOI params as literals there, which is not dim-check-clean — bench_scaling
    # uses --no-dim-check for that contrast; the rate_grad multiplier is already
    # documented in the FOI study and visible in the IR-size curve.)
    pts.append(dict(P=16, A=7, coupling="off", grad="full", slice="axis"))
    return pts


def _timed(cmd: list[str], env: dict) -> tuple[float, int, int]:
    """Run `cmd` under /usr/bin/time -l. Returns (real_s, maxrss_bytes, rc)."""
    full = ["/usr/bin/time", "-l", *cmd]
    proc = subprocess.run(full, env=env, capture_output=True, text=True)
    err = proc.stderr
    real = float(m.group(1)) if (m := _REAL.search(err)) else float("nan")
    rss = int(m.group(1)) if (m := _MAXRSS.search(err)) else -1
    return real, rss, proc.returncode


def bench_one(camdlc: str, camdl_path: Path, out_ir: Path, env: dict,
              reps: int, no_dim_check: bool) -> dict | None:
    """Compile `camdl_path` `reps` times; return timing/RSS aggregates.

    wall is reported as min (cleanest, least scheduler noise) and median;
    peak RSS is the max over reps (worst case); ir_bytes from the last run.
    Returns None on any compile failure.
    """
    cmd = [camdlc, str(camdl_path), "-o", str(out_ir)]
    if no_dim_check:
        cmd.append("--no-dim-check")
    walls: list[float] = []
    rss_peak = -1
    ir_bytes = -1
    for _ in range(reps):
        wall, rss, rc = _timed(cmd, env)
        if rc != 0 or not out_ir.exists():
            return None
        walls.append(wall)
        rss_peak = max(rss_peak, rss)
        ir_bytes = out_ir.stat().st_size
        out_ir.unlink(missing_ok=True)  # bound transient disk (IR can be GBs)
    return dict(
        wall_min=min(walls),
        wall_median=statistics.median(walls),
        peak_rss_mb=rss_peak / 1e6 if rss_peak > 0 else float("nan"),
        ir_bytes=ir_bytes,
        reps=reps,
    )


COLS = ["label", "slice", "P", "A", "coupling", "grad",
        "n_compartments", "n_transitions", "ir_bytes",
        "wall_min_s", "wall_median_s", "peak_rss_mb", "reps"]

PASS_COLS = ["label", "slice", "P", "A", "ir_bytes", *PASSES, "total_s"]


def pass_breakdown(camdlc: str, camdl_path: Path, out_ir: Path,
                   env: dict, no_dim_check: bool) -> dict[str, float] | None:
    """One compile under CAMDL_TIME_PASSES; parse per-pass processor seconds."""
    cmd = [camdlc, str(camdl_path), "-o", str(out_ir)]
    if no_dim_check:
        cmd.append("--no-dim-check")
    proc = subprocess.run(cmd, env={**env, "CAMDL_TIME_PASSES": "1"},
                          capture_output=True, text=True)
    out_ir.unlink(missing_ok=True)
    if proc.returncode != 0:
        return None
    found = {p: 0.0 for p in PASSES}
    for name, secs in _PASS_LINE.findall(proc.stderr):
        if name in found:
            found[name] = float(secs)
    return found


def run(out_tsv: Path, reps: int, no_dim_check: bool, camdlc: str,
        reals: list[Path], rss_ceiling_gb: float,
        passes_tsv: Path | None) -> list[list]:
    if not Path(camdlc).exists():
        sys.exit(f"camdlc not found at {camdlc} — run `make build-ocaml` first")
    WORK.mkdir(parents=True, exist_ok=True)
    out_tsv.parent.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "TMPDIR": "/tmp"}

    rows: list[list] = []
    pass_rows: list[list] = []

    def emit() -> None:
        with out_tsv.open("w") as f:
            f.write("\t".join(COLS) + "\n")
            for r in rows:
                f.write("\t".join(str(x) for x in r) + "\n")
        if passes_tsv is not None:
            with passes_tsv.open("w") as f:
                f.write("\t".join(PASS_COLS) + "\n")
                for r in pass_rows:
                    f.write("\t".join(str(x) for x in r) + "\n")

    def record(label: str, sl: str, P, A, coup, grad,
               ncomp, ntr, camdl_path: Path) -> dict | None:
        res = bench_one(camdlc, camdl_path, WORK / f"{label}.ir.json",
                        env, reps, no_dim_check)
        if res is None:
            print("COMPILE FAIL", file=sys.stderr)
            return None
        rows.append([label, sl, P, A, coup, grad, ncomp, ntr, res["ir_bytes"],
                     f"{res['wall_min']:.3f}", f"{res['wall_median']:.3f}",
                     f"{res['peak_rss_mb']:.1f}", reps])
        if passes_tsv is not None:
            bd = pass_breakdown(camdlc, camdl_path, WORK / f"{label}.ir.json",
                                env, no_dim_check)
            if bd is not None:
                total = sum(bd.values())
                pass_rows.append([label, sl, P, A, res["ir_bytes"],
                                  *[f"{bd[p]:.3f}" for p in PASSES],
                                  f"{total:.3f}"])
        emit()
        print(f"ir={res['ir_bytes']/1e6:7.1f}MB wall={res['wall_min']:6.2f}s "
              f"rss={res['peak_rss_mb']/1000:5.2f}GB", file=sys.stderr)
        return res

    # Synthetic ladder (smallest first; abort the ladder if a point blows the
    # RSS ceiling so we never push the machine toward the OOM-watchdog regime).
    pts = synthetic_grid()
    print(f"compile sweep: {len(pts)} synthetic + {len(reals)} real "
          f"-> {out_tsv}"
          + (f" (+per-pass -> {passes_tsv})" if passes_tsv else ""),
          file=sys.stderr)
    for i, p in enumerate(pts, 1):
        P, A, coup, grad = p["P"], p["A"], p["coupling"], p["grad"]
        label = f"P{P}_A{A}_{coup}_{grad}"
        camdl_f = WORK / f"{label}.camdl"
        camdl_f.write_text(gen_camdl(P, A, coup, grad))
        print(f"[{i}/{len(pts)}] {p['slice']:5s} {label} ... ",
              end="", file=sys.stderr, flush=True)
        res = record(label, p["slice"], P, A, coup, grad,
                     4 * P * A, 3 * P * A, camdl_f)
        if res is not None and res["peak_rss_mb"] / 1000 > rss_ceiling_gb:
            print(f"  ! peak RSS {res['peak_rss_mb']/1000:.1f}GB > ceiling "
                  f"{rss_ceiling_gb}GB — stopping synthetic ladder",
                  file=sys.stderr)
            break

    # Real models (timed as-is; structural counts left blank — read off the
    # model / inspect summary separately, ir_bytes is the size proxy here).
    for rp in reals:
        if not rp.exists():
            print(f"  ! real model not found: {rp} (skipped)", file=sys.stderr)
            continue
        print(f"[real] {rp.stem} ... ", end="", file=sys.stderr, flush=True)
        record(rp.stem, "real", "", "", "", "", "", "", rp)

    print(f"done: {len(rows)} rows -> {out_tsv}", file=sys.stderr)
    return rows


def markdown_table(rows: list[list]) -> str:
    """Render rows as a GitHub-flavoured markdown table for the baseline note."""
    hdr = ["model", "n_comp", "n_tr", "IR (MB)", "wall min (s)",
           "wall med (s)", "peak RSS (GB)"]
    out = ["| " + " | ".join(hdr) + " |",
           "|" + "|".join(["---"] * len(hdr)) + "|"]
    for r in rows:
        (label, _sl, _P, _A, _c, _g, ncomp, ntr, irb,
         wmin, wmed, rss, _reps) = r
        out.append("| " + " | ".join([
            f"`{label}`", str(ncomp), str(ntr),
            f"{int(irb)/1e6:.1f}", wmin, wmed, f"{float(rss)/1000:.2f}",
        ]) + " |")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("-o", "--out", type=Path,
                    default=REPO / "docs/dev/notes/assets/compile/compile_baseline.tsv")
    ap.add_argument("--reps", type=int, default=3, help="repetitions per point")
    ap.add_argument("--no-dim-check", action="store_true",
                    help="pass --no-dim-check to camdlc (isolates the dimcheck pass)")
    ap.add_argument("--camdlc", default=os.environ.get("CAMDLC", str(DEFAULT_CAMDLC)))
    ap.add_argument("--real", type=Path, action="append", default=[],
                    help="real .camdl model(s) to time alongside the ladder")
    ap.add_argument("--rss-ceiling-gb", type=float, default=20.0,
                    help="abort the synthetic ladder if a point exceeds this RSS")
    ap.add_argument("--passes", action="store_true",
                    help="also capture the per-pass CAMDL_TIME_PASSES breakdown "
                         "into <out>_passes.tsv (one extra compile per point)")
    args = ap.parse_args()
    passes_tsv = (args.out.with_name(args.out.stem + "_passes.tsv")
                  if args.passes else None)
    t0 = time.time()
    rows = run(args.out, args.reps, args.no_dim_check, args.camdlc,
               args.real, args.rss_ceiling_gb, passes_tsv)
    print(f"\n<!-- bench_compile.py: {len(rows)} points in "
          f"{time.time() - t0:.0f}s, reps={args.reps}, "
          f"dim_check={'off' if args.no_dim_check else 'on'} -->")
    print(markdown_table(rows))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
