#!/usr/bin/env python3
"""Pre-resolution A/B: string-keyed eval_expr (before) vs indexed eval_resolved
(after), and how the win compounds with model coupling.

Panel A — per-eval cost (microbench, micro_eval_ab.tsv): grouped bars of median
ns/eval, before (unresolved) vs after (resolved), log-y, with the speedup factor
k annotated. Panel B — end-to-end pfilter speedup vs eval's share of the inner
loop: the win is ~1x when eval is a sliver and ~10x+ once the O(P^2*A) coupling
sum makes eval dominate (national-scale regime).

    uv run --with matplotlib --with numpy scripts/plot_eval_ab.py

Writes docs/dev/notes/assets/eval-ab/eval_ab_before_after.png
"""
from __future__ import annotations

import csv
import statistics
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
ASSETS = REPO / "docs/dev/notes/assets/eval-ab"

# Display order (toy -> coupled) + the end-to-end speedup & eval-fraction story.
# end_to_end / eval_frac filled from the harness TSVs where measured; P16A7's
# eval_frac anchored to the 2026-05-29 PMMH flamegraph (~72%).
MODELS = [
    ("sir_basic",                 "SIR\n(2 tr)"),
    ("seir_observations",         "SEIR obs\n(3 tr)"),
    ("seir_age",                  "SEIR age\n(6 tr)"),
    ("seir_spatial_5_inference",  "spatial P=5\n(40 tr)"),
    ("pmmh_P16A7",                "spatial P=16,A=7\n(336 tr)"),
]


def load_micro(path: Path) -> dict:
    rows = list(csv.DictReader(path.open(), delimiter="\t"))
    out: dict[str, dict] = {}
    for r in rows:
        m = out.setdefault(r["model"], {"resolved": [], "unresolved": [],
                                        "probes": float(r["probes_per_eval"]),
                                        "n_tr": int(r["n_transitions"])})
        m[r["kind"]].append(float(r["ns_per_eval"]))
    for m in out.values():
        m["res"] = statistics.median(m["resolved"])
        m["unr"] = statistics.median(m["unresolved"])
        m["k"] = m["unr"] / m["res"]
    return out


def scale_e2e(path: Path) -> dict:
    """median raw ratio at the largest particle count, per model."""
    if not path.exists():
        return {}
    rows = list(csv.DictReader(path.open(), delimiter="\t"))
    out: dict[str, dict] = {}
    for r in rows:
        out.setdefault(r["model"], {}).setdefault((r["switch"], int(r["particles"])), []).append(float(r["secs"]))
    res = {}
    for model, d in out.items():
        pmax = max(p for (_, p) in d)
        ro = statistics.median(d[("resolved", pmax)])
        un = statistics.median(d[("unresolved", pmax)])
        res[model] = un / ro
    return res


def main() -> int:
    micro = load_micro(ASSETS / "micro_eval_ab.tsv")
    scale = scale_e2e(ASSETS / "end_to_end_scale_runs.tsv")

    # end-to-end speedup + eval-fraction per model (measured where available).
    # golden pair: from end_to_end_summary.tsv (marginal inner-loop ratio).
    e2e = {}
    frac = {}
    summ = ASSETS / "end_to_end_summary.tsv"
    if summ.exists():
        for r in csv.DictReader(summ.open(), delimiter="\t"):
            e2e[r["model"]] = float(r["marginal_ratio"])
            try:
                frac[r["model"]] = float(r["eval_frac_derived"])
            except ValueError:
                pass
    e2e.update(scale)                          # P16A7 raw ratio
    frac.setdefault("pmmh_P16A7", 0.72)        # 2026-05-29 PMMH flamegraph
    frac.setdefault("sir_basic", None)

    present = [(k, lab) for (k, lab) in MODELS if k in micro]
    labels = [lab for _, lab in present]
    res = [micro[k]["res"] for k, _ in present]
    unr = [micro[k]["unr"] for k, _ in present]
    ks = [micro[k]["k"] for k, _ in present]

    fig, (axA, axB) = plt.subplots(1, 2, figsize=(12.5, 4.8))

    # ── Panel A: per-eval before/after ─────────────────────────────────────
    x = range(len(present))
    w = 0.38
    axA.bar([i - w / 2 for i in x], unr, w, label="before: eval_expr (string-keyed)",
            color="#c44e52")
    axA.bar([i + w / 2 for i in x], res, w, label="after: eval_resolved (indexed)",
            color="#4c72b0")
    axA.set_yscale("log")
    axA.set_ylabel("ns per rate eval (median)")
    axA.set_xticks(list(x))
    axA.set_xticklabels(labels, fontsize=8)
    axA.set_title("A. Per-eval cost: string-keyed vs pre-resolved")
    for i, k in zip(x, ks):
        top = max(unr[i], res[i])
        axA.annotate(f"{k:.1f}×", (i, top * 1.25), ha="center", fontsize=9, fontweight="bold")
    axA.legend(fontsize=8, loc="upper left")
    axA.margins(y=0.18)

    # ── Panel B: end-to-end speedup vs eval's share of the loop ─────────────
    # The win = 1 + f*(k-1): grows with eval's share f AND with per-eval k,
    # both of which climb with coupling. Bracket with guide curves k=4 (toy)
    # and the largest observed k; annotate each point with its own k.
    fg = [i / 100 for i in range(0, 101)]
    for kg, sty in [(4, ":"), (max(ks), "--")]:
        axB.plot([f * 100 for f in fg], [1 + f * (kg - 1) for f in fg], sty,
                 color="grey", lw=1, label=f"1 + f·(k−1), k={kg:.0f}")
    for k, lab in present:
        f, s = frac.get(k), e2e.get(k)
        if f is None or s is None:
            continue
        axB.scatter([f * 100], [s], s=70, color="#55a868", zorder=3)
        axB.annotate(f"{lab.splitlines()[0]}\nk={micro[k]['k']:.0f}",
                     (f * 100, s), textcoords="offset points", xytext=(7, -2), fontsize=8)
    axB.set_yscale("log")
    axB.set_xlabel("eval share of inner loop  (%)")
    axB.set_ylabel("end-to-end pfilter speedup (×)")
    axB.set_title("B. The win compounds with coupling")
    axB.axhline(1.0, color="black", lw=0.6)
    axB.legend(fontsize=8, loc="upper left")
    axB.margins(0.12)

    fig.suptitle("Pre-resolution (eval_resolved) vs string-keyed eval_expr — bit-identical, "
                 "verified on gate_trajectory_baseline", fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    out = ASSETS / "eval_ab_before_after.png"
    fig.savefig(out, dpi=140)
    print(f"wrote {out}")
    print("per-eval k:", {k: round(micro[k]["k"], 2) for k, _ in present})
    print("end-to-end:", {k: round(e2e[k], 2) for k, _ in present if k in e2e})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
