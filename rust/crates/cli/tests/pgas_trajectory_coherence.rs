//! gh#264: a saved PGAS trajectory must be a single directionally-coherent
//! path — `S` monotone non-increasing, each step's `−ΔS` equal to the
//! infection flow, and population conserved. This is the END-TO-END guard:
//! it runs a tiny PGAS fit and audits a written `trajectory_*.tsv`. (The
//! reconstruction logic itself is unit-tested in
//! `sim::inference::pgas::coherent_counts_after_removes_as_join_backflow`;
//! this test covers the writer wiring + output coherence so a regression in
//! the serialization can't slip through.)
//!
//! Skipped when the release binary or camdlc isn't present.

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let p = Path::new(&manifest).join("../../target/release/camdl");
    assert!(p.exists(), "release camdl binary missing: {} — run `make build-rust`", p.display());
    p
}

fn camdlc_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../../ocaml/_build/default/bin/camdlc.exe");
    if p.exists() { Some(p) } else { None }
}

fn find_one_trajectory(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("trajectory_") && n.ends_with(".tsv")
            }) {
                return Some(p);
            }
        }
    }
    None
}

#[test]
fn saved_pgas_trajectory_is_directionally_coherent() {
    let bin = camdl_bin();
    let Some(camdlc) = camdlc_bin() else { return };
    let tmp = std::env::temp_dir().join(format!("camdl_traj_coh_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    // Tiny SIR — S leaves only via infection, so a coherent path has S
    // monotone non-increasing and −ΔS == flow_infection at every step.
    let src = r#"
time_unit = 'days
compartments { S, I, R }
parameters {
  beta  : rate  in [0.001, 5.0]
  gamma : rate  in [0.01, 1.0]
  N0    : count in [100, 10000]
}
transitions {
  infection : S --> I @ beta * S * I / N0
  recovery  : I --> R @ gamma * I
}
observations {
  cases {
    columns       { time : time, cases : count }
    projected  = prevalence(I)
    emit_schedule = every 1 'days
    cases ~ poisson(rate = projected)
  }
}
init { S = 999  I = 1 }
simulate { from = 0 'days  to = 12 'days }
"#;
    let model = tmp.join("sir.camdl");
    std::fs::write(&model, src).unwrap();
    let ir = tmp.join("sir.ir.json");
    let out = Command::new(&camdlc).arg(&model).output().unwrap();
    assert!(out.status.success(), "camdlc failed: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(&ir, &out.stdout).unwrap();
    std::fs::write(
        tmp.join("cases.tsv"),
        "time\tcases\n1\t2\n2\t4\n3\t8\n4\t12\n5\t9\n6\t7\n7\t5\n8\t4\n9\t3\n10\t2\n11\t1\n12\t1\n",
    )
    .unwrap();

    let fit = tmp.join("fit.toml");
    std::fs::write(&fit, format!(r#"
output_dir = "{out}"
[model]
camdl = "{ir}"
[data.observations]
cases = "{data}"
[config]
dt = 1.0
[estimate]
beta  = {{ bounds = [0.01, 5.0], prior = {{ log_normal = {{ mu = -0.3, sigma = 0.5 }} }}, start = 0.8 }}
gamma = {{ bounds = [0.01, 1.0], prior = {{ log_normal = {{ mu = -1.2, sigma = 0.5 }} }}, start = 0.3 }}
[fixed]
N0 = 1000
[stages.post]
algorithm = "pgas"
backend = "chain_binomial"
chains = 1
particles = 30
sweeps = 12
burn_in = 2
n_trajectories = 4
"#,
        out = tmp.join("results").display(),
        ir = ir.display(),
        data = tmp.join("cases.tsv").display(),
    )).unwrap();

    let r = Command::new(&bin)
        .arg("fit").arg("run").arg(&fit).arg("--seed").arg("1")
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .output().expect("spawn");
    assert!(r.status.success(), "fit run failed: {}", String::from_utf8_lossy(&r.stderr));

    let traj = find_one_trajectory(&tmp.join("results"))
        .expect("a saved trajectory_*.tsv under the fit results");
    let text = std::fs::read_to_string(&traj).unwrap();
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("header").split('\t').collect();
    let col = |name: &str| header.iter().position(|h| *h == name)
        .unwrap_or_else(|| panic!("column {name} not in header {header:?}"));
    let (si, ii, ri, fi) = (col("S"), col("I"), col("R"), col("flow_infection"));

    let mut prev_s: Option<i64> = None;
    let mut pop0: Option<i64> = None;
    let mut rows = 0usize;
    for line in lines {
        let c: Vec<i64> = line.split('\t')
            .map(|v| v.parse::<f64>().unwrap_or(0.0) as i64).collect();
        let (s, inf) = (c[si], c[fi]);
        let pop = c[si] + c[ii] + c[ri];
        if let Some(ps) = prev_s {
            assert!(s <= ps, "S must be monotone non-increasing, got {ps} -> {s}");
            assert_eq!(ps - s, inf, "−ΔS must equal flow_infection (coherent path)");
        }
        let p0 = *pop0.get_or_insert(pop);
        assert_eq!(pop, p0, "S+I+R must be conserved across the path");
        prev_s = Some(s);
        rows += 1;
    }
    assert!(rows > 0, "the saved trajectory had no rows");
    let _ = std::fs::remove_dir_all(&tmp);
}
