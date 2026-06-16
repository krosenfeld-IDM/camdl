#!/usr/bin/env python3
"""gh#166 Phase B incidence oracle — scipy solve_ivp reference.

Encodes each canonical model's ODE RHS *independently* of camdl, augmented with
one cumulative-incidence variable per tracked transition (dc_i/dt = rate_i).
Integrates with method="RK45" (Dormand-Prince, the same family as camdl's Phase
C rk45) and method="LSODA" (an independent adaptive stiff/non-stiff reference),
samples at the model's output grid, and writes per-interval incidence +
prevalence to ref/<model>__<method>.tsv.

Params here MUST match the `--set` values used to compile models/<model>.ir.json.
Run via the regen wrapper:  bash tests/external/ode_oracle/gen/run.sh
(or:  uv run --with scipy --with numpy python scipy_oracle.py)

CI never runs this — it reads the committed ref/*.tsv fixtures.
"""
import os
import numpy as np
from scipy.integrate import solve_ivp

REF = os.path.join(os.path.dirname(__file__), "..", "ref")

# RHS functions. y = [<compartments...>, <cumulative incidence...>] in the same
# order as the compiled IR's compartments / transitions.

def sir_rhs(t, y, p):
    S, I, R, _ci, _cr = y
    N = S + I + R
    inf = p["beta"] * S * I / N
    rec = p["gamma"] * I
    return [-inf, inf - rec, rec, inf, rec]

def seir_rhs(t, y, p):
    S, E, I, R, _ci, _cp, _cr = y
    N = S + E + I + R
    inf = p["beta"] * S * I / N
    prog = p["sigma"] * E
    rec = p["gamma"] * I
    return [-inf, inf - prog, prog - rec, rec, inf, prog, rec]

def tb_rhs(t, y, p):
    S, Lf, Ls, I, R, _i, _fp, _st, _re, _rc = y
    N = S + Lf + Ls + I + R
    inf = p["beta"] * S * I / N
    fp = p["phi"] * Lf
    st = p["kappa"] * Lf
    re = p["omega"] * Ls
    rec = p["gamma"] * I
    return [-inf, inf - (fp + st), st - re, fp + re - rec, rec, inf, fp, st, re, rec]

MODELS = {
    "sir": dict(
        rhs=sir_rhs, comps=["S", "I", "R"], incs=["infection", "recovery"],
        params=dict(beta=0.5, gamma=0.25, N0=100000.0, I0=10.0),
        y0=lambda p: [p["N0"] - p["I0"], p["I0"], 0.0, 0.0, 0.0],
        t_end=60.0, step=1.0,
    ),
    "seir": dict(
        rhs=seir_rhs, comps=["S", "E", "I", "R"], incs=["infection", "progression", "recovery"],
        params=dict(beta=0.6, sigma=0.2, gamma=0.25, N0=100000.0, I0=10.0),
        y0=lambda p: [p["N0"] - p["I0"], 0.0, p["I0"], 0.0, 0.0, 0.0, 0.0],
        t_end=80.0, step=1.0,
    ),
    "tb": dict(
        rhs=tb_rhs, comps=["S", "Lf", "Ls", "I", "R"],
        incs=["infection", "fast_prog", "stabilize", "reactivate", "recovery"],
        params=dict(beta=0.3, phi=0.02, kappa=0.008, omega=0.0003, gamma=0.02, N0=100000.0, I0=50.0),
        y0=lambda p: [p["N0"] - p["I0"], 0.0, 0.0, p["I0"], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        t_end=365.0, step=1.0,
    ),
}

METHODS = {"scipy_rk45": "RK45", "scipy_lsoda": "LSODA"}


def main():
    os.makedirs(REF, exist_ok=True)
    for name, m in MODELS.items():
        p = m["params"]
        nc, ni = len(m["comps"]), len(m["incs"])
        grid = np.arange(0.0, m["t_end"] + 0.5 * m["step"], m["step"])
        y0 = m["y0"](p)
        for tag, method in METHODS.items():
            sol = solve_ivp(
                lambda t, y: m["rhs"](t, y, p),
                [0.0, m["t_end"]], y0, method=method, t_eval=grid,
                rtol=1e-11, atol=1e-11,
            )
            assert sol.success, f"{name}/{method}: {sol.message}"
            Y = sol.y  # (nvars, ngrid)
            comps = Y[:nc, :]
            cum = Y[nc:nc + ni, :]
            per = np.zeros_like(cum)
            per[:, 1:] = cum[:, 1:] - cum[:, :-1]  # per-interval incidence

            path = os.path.join(REF, f"{name}__{tag}.tsv")
            header = ["t"] + m["comps"] + [f"inc_{x}" for x in m["incs"]]
            with open(path, "w") as fh:
                fh.write("\t".join(header) + "\n")
                for k, t in enumerate(grid):
                    row = [f"{t:.1f}"]
                    row += [f"{comps[c, k]:.10e}" for c in range(nc)]
                    row += [f"{per[i, k]:.10e}" for i in range(ni)]
                    fh.write("\t".join(row) + "\n")
            print(f"wrote {path} ({len(grid)} rows)")


if __name__ == "__main__":
    main()
