#!/usr/bin/env python3
"""End-to-end A/B: time `camdl pfilter` with the propensity eval routed
through the pre-resolved index path (default) vs the string-keyed eval_expr
path (CAMDL_EVAL_UNRESOLVED=1).

Times each model at two particle counts so the per-particle *marginal* cost
(the slope) cancels fixed startup (compile + data load). The switch only
affects the per-particle inner loop, so the marginal ratio is the clean
inner-loop speedup; the raw ratio at the high count is the whole-run speedup.

Emits:
  end_to_end_runs.tsv     one row per (model, switch, particles, rep)
  end_to_end_summary.tsv  per-model marginal/raw speedups + derived eval fraction

The eval fraction follows from  T_on/T_off = 1 + f*(k-1)  =>  f = (ratio-1)/(k-1),
with k the per-eval microbench factor (eval_ab example) passed in via --k.
"""
import argparse, json, os, statistics, subprocess, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]          # .../compiler-profiling
RUST = REPO / "rust"
CAMDL = RUST / "target" / "release" / "camdl"
GOLDEN = REPO / "ocaml" / "golden"

# (label, model, scenario, [--data specs], microbench k from eval_ab)
MODELS = [
    ("seir_observations", "seir_observations", "baseline",
     ["weekly_cases={obs}/seir_obs/weekly_cases.tsv"], None),
    ("seir_spatial_5_inference", "seir_spatial_5_inference", "true_params",
     [f"cases_p{i}={{obs}}/spatial/cases_p{i}.tsv" for i in range(1, 6)], None),
]


def run_once(model, scenario, datas, particles, unresolved, seed=1):
    env = dict(os.environ, CAMDL_SKIP_VERSION_CHECK="1")
    if unresolved:
        env["CAMDL_EVAL_UNRESOLVED"] = "1"
    else:
        env.pop("CAMDL_EVAL_UNRESOLVED", None)
    cmd = [str(CAMDL), "pfilter", str(GOLDEN / f"{model}.ir.json"),
           "--scenario", scenario, "--particles", str(particles),
           "--dt", "1", "--seed", str(seed)]
    for d in datas:
        cmd += ["--data", d]
    t0 = time.perf_counter()
    r = subprocess.run(cmd, env=env, capture_output=True, text=True)
    dt = time.perf_counter() - t0
    if r.returncode != 0:
        raise RuntimeError(f"pfilter failed: {' '.join(cmd)}\n{r.stderr[-500:]}")
    return dt


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--obs", default="/tmp/eval_ab")
    ap.add_argument("--p-low", type=int, default=2000)
    ap.add_argument("--p-high", type=int, default=20000)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--k", default="", help="comma list label=k from eval_ab microbench")
    args = ap.parse_args()

    kmap = {}
    for tok in args.k.split(","):
        if "=" in tok:
            lab, v = tok.split("=", 1); kmap[lab] = float(v)

    runs = []           # (label, switch, particles, rep, secs)
    for label, model, scenario, datas_t, _ in MODELS:
        datas = [d.format(obs=args.obs) for d in datas_t]
        for switch, unres in [("resolved", False), ("unresolved", True)]:
            for p in (args.p_low, args.p_high):
                # one warmup (discarded), then timed reps
                run_once(model, scenario, datas, p, unres)
                for rep in range(args.reps):
                    secs = run_once(model, scenario, datas, p, unres, seed=1 + rep)
                    runs.append((label, switch, p, rep, secs))
                    print(f"{label:26s} {switch:11s} p={p:<6d} rep={rep} {secs:7.3f}s")

    with open(HERE / "end_to_end_runs.tsv", "w") as f:
        f.write("model\tswitch\tparticles\trep\tsecs\n")
        for r in runs:
            f.write("\t".join(str(x) for x in r) + "\n")

    # ── summary: median per (model, switch, particles); slope = marginal cost ──
    def med(label, switch, p):
        xs = [s for (l, sw, pp, _, s) in runs if l == label and sw == switch and pp == p]
        return statistics.median(xs)

    lines = ["model\tk_micro\tT_off_high\tT_on_high\traw_ratio\t"
             "c_off_ns_per_particle\tc_on_ns_per_particle\tmarginal_ratio\teval_frac_derived"]
    summ = []
    for label, *_ in MODELS:
        lo, hi = args.p_low, args.p_high
        off_lo, off_hi = med(label, "resolved", lo), med(label, "resolved", hi)
        on_lo, on_hi = med(label, "unresolved", lo), med(label, "unresolved", hi)
        c_off = (off_hi - off_lo) / (hi - lo)       # secs per particle (full sweep)
        c_on = (on_hi - on_lo) / (hi - lo)
        raw_ratio = on_hi / off_hi
        marg_ratio = c_on / c_off
        k = kmap.get(label)
        frac = ((marg_ratio - 1) / (k - 1)) if k and k > 1 else float("nan")
        lines.append(f"{label}\t{k if k else 'NA'}\t{off_hi:.3f}\t{on_hi:.3f}\t{raw_ratio:.3f}\t"
                     f"{c_off*1e9:.1f}\t{c_on*1e9:.1f}\t{marg_ratio:.3f}\t{frac:.3f}")
        summ.append((label, k, raw_ratio, marg_ratio, frac))
    (HERE / "end_to_end_summary.tsv").write_text("\n".join(lines) + "\n")

    print("\n── summary ─────────────────────────────────────")
    for label, k, raw, marg, frac in summ:
        print(f"  {label:26s} k={k}  raw={raw:.2f}x  marginal(inner-loop)={marg:.2f}x  "
              f"eval_frac≈{frac:.0%}")
    print(f"\nwrote {HERE/'end_to_end_runs.tsv'} and {HERE/'end_to_end_summary.tsv'}")


if __name__ == "__main__":
    main()
