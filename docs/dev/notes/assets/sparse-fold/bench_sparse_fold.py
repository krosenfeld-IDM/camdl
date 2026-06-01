#!/usr/bin/env python3
"""A/B for the sparse-coupling constant-fold pass (CAMDL_CONSTANT_FOLD).

For each P (fixed neighbour degree k), generate a sparse-ring + guarded-FOI
model, then compile it BOTH ways (fold off = dense P-term FOI Reduce; fold on
= k-term Reduce) and measure:
  - IR size (bytes)            — the compile/parse/RAM win
  - compile wall (camdlc)      — fold adds a pass but emits smaller IR
  - pfilter wall (fixed N)     — the runtime inner-loop win (fewer eval terms)
  - bit-exactness              — simulate both, assert byte-identical trajectory

Writes sparse_fold.tsv (one row per P) + prints a summary.

  python3 bench_sparse_fold.py --P 16,32,64 --k 4 --particles 1500 --reps 3
"""
import argparse, hashlib, os, statistics, subprocess, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]
RUST = REPO / "rust"
CAMDL = RUST / "target" / "release" / "camdl"
CAMDLC = REPO / "ocaml" / "_build" / "default" / "bin" / "camdlc.exe"
GEN = REPO / "scripts" / "gen_scaling_models.py"
ENV = dict(os.environ, CAMDL_SKIP_VERSION_CHECK="1")


def sh(cmd, env=ENV):
    r = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"FAILED: {' '.join(map(str,cmd))}\n{r.stderr[-600:]}")
    return r


def compile_timed(camdl_path, ir_path, fold):
    env = dict(ENV)
    if fold: env["CAMDL_CONSTANT_FOLD"] = "1"
    else: env.pop("CAMDL_CONSTANT_FOLD", None)
    t0 = time.perf_counter()
    sh([str(CAMDLC), str(camdl_path), "--no-dim-check", "-o", str(ir_path)], env=env)
    return time.perf_counter() - t0


def md5(path):
    return hashlib.md5(Path(path).read_bytes()).hexdigest()


def simulate(ir, out):
    sh([str(CAMDL), "simulate", str(ir), "--backend", "chain_binomial", "--dt", "1",
        "--seed", "42", "--scenario", "baseline", "--output", str(out)])


def pfilter_timed(ir, data, particles, reps):
    cmd = [str(CAMDL), "pfilter", str(ir), "--scenario", "baseline",
           "--data", f"weekly_cases={data}", "--particles", str(particles),
           "--dt", "1", "--seed", "1"]
    sh(cmd)  # warmup
    ts = []
    for r in range(reps):
        t0 = time.perf_counter(); sh(cmd); ts.append(time.perf_counter() - t0)
    return statistics.median(ts)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--P", default="16,32,64")
    ap.add_argument("--k", type=int, default=4)
    ap.add_argument("--A", type=int, default=1)
    ap.add_argument("--particles", type=int, default=1500)
    ap.add_argument("--reps", type=int, default=3)
    args = ap.parse_args()
    Ps = [int(x) for x in args.P.split(",")]

    tmp = Path("/tmp/sparse_fold"); tmp.mkdir(exist_ok=True)
    rows = []
    for P in Ps:
        cam = tmp / f"sp_P{P}.camdl"
        sh(["python3", str(GEN), "-P", str(P), "-A", str(args.A), "--coupling", "on",
            "--grad", "full", "--observe", "--coupling-degree", str(args.k), "-o", str(cam)])
        ir_off, ir_on = tmp / f"sp_P{P}_off.ir.json", tmp / f"sp_P{P}_on.ir.json"
        c_off = compile_timed(cam, ir_off, fold=False)
        c_on = compile_timed(cam, ir_on, fold=True)
        sz_off, sz_on = ir_off.stat().st_size, ir_on.stat().st_size

        # bit-exactness: simulate both, compare trajectory md5
        t_off, t_on = tmp / f"tr_P{P}_off.tsv", tmp / f"tr_P{P}_on.tsv"
        simulate(ir_off, t_off); simulate(ir_on, t_on)
        bitexact = md5(t_off) == md5(t_on)

        # synth obs once (from off), pfilter both ways
        obsdir = tmp / f"obs_P{P}"; obsdir.mkdir(exist_ok=True)
        sh([str(CAMDL), "simulate", str(ir_off), "--backend", "chain_binomial", "--dt", "1",
            "--seed", "42", "--scenario", "baseline", "--obs-dir", str(obsdir)])
        data = obsdir / "weekly_cases.tsv"
        pf_off = pfilter_timed(ir_off, data, args.particles, args.reps)
        pf_on = pfilter_timed(ir_on, data, args.particles, args.reps)

        rows.append(dict(P=P, k=args.k, ir_off=sz_off, ir_on=sz_on,
                         comp_off=c_off, comp_on=c_on, pf_off=pf_off, pf_on=pf_on,
                         bitexact=bitexact))
        print(f"P={P:4d}  IR {sz_off/1e6:6.2f}->{sz_on/1e6:5.2f} MB ({sz_off/sz_on:4.1f}x)  "
              f"compile {c_off:5.2f}->{c_on:5.2f}s  pfilter {pf_off:6.2f}->{pf_on:5.2f}s "
              f"({pf_off/pf_on:4.1f}x)  bitexact={'YES' if bitexact else 'NO'}")

    out = HERE / "sparse_fold.tsv"
    with open(out, "w") as f:
        f.write("P\tk\tir_off\tir_on\tcompile_off\tcompile_on\tpfilter_off\tpfilter_on\tbitexact\n")
        for r in rows:
            f.write(f"{r['P']}\t{r['k']}\t{r['ir_off']}\t{r['ir_on']}\t{r['comp_off']:.4f}\t"
                    f"{r['comp_on']:.4f}\t{r['pf_off']:.4f}\t{r['pf_on']:.4f}\t{int(r['bitexact'])}\n")
    print(f"\nwrote {out}")
    print("bit-exact all P:", all(r["bitexact"] for r in rows))


if __name__ == "__main__":
    main()
