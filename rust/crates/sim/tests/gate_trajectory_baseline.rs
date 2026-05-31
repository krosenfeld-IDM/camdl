//! Byte-identical trajectory baseline gate (the refactor ratchet).
//!
//! For every `ocaml/golden/*.ir.json`, simulate under each supported backend at a
//! fixed seed and assert the full trajectory hashes to a committed baseline. This
//! is the gate for behavior-preserving compiler/runtime refactors (the shared-
//! bindings + reduction work, docs/dev/proposals/2026-05-29-shared-bindings-and-
//! reduction.md): if D/B1 perturb a single count, this fails loudly and names the
//! model+backend, rather than the change passing on associativity-blind small
//! goldens (a 3-term `N=S+I+R` sums identically in any order; only a large/mixed
//! sum exposes a reassociation regression — see the gate models added alongside).
//!
//! Baselines are machine/toolchain-specific (libm `exp`/`sqrt` can differ by a
//! ULP across platforms). This is a *development ratchet*: capture on the dev
//! machine, run before/after each refactor phase on the same machine. Re-capture
//! with `CAMDL_CAPTURE_BASELINE=1 cargo test -p sim --test gate_trajectory_baseline
//! -- --nocapture` and paste the printed table below.
//!
//! Mirrors smoke_all_golden.rs (discovery, backend matrix, capability-skip) but
//! asserts trajectory identity, not just invariants.

use std::path::PathBuf;
use sim::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, GillespieConfig, OdeConfig, SimConfig, TauLeapConfig},
    simulate::Simulate,
    ChainBinomialSim, GillespieSim, OdeSim, TauLeapSim,
};

const SEED: u64 = 42;

fn ocaml_golden_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(&manifest).join("../../../ocaml/golden")
}

fn discover_models() -> Vec<String> {
    let dir = ocaml_golden_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {:?}: {}", dir, e))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".ir.json").map(|s| s.to_owned())
        })
        .collect();
    names.sort();
    names
}

fn load_and_apply_baseline(name: &str) -> ir::Model {
    let path = ocaml_golden_dir().join(format!("{}.ir.json", name));
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {:?}: {}", path, e));
    let mut model: ir::Model = ir::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", name, e));
    if let Some(preset) = model.presets.first().cloned() {
        for p in &mut model.parameters {
            if let Some(&v) = preset.params.get(&p.name) {
                p.value = Some(v);
            }
        }
    }
    model
}

/// FNV-1a/64 over the full trajectory numeric content. Deterministic and
/// platform-independent given identical inputs (no std hasher RNG).
fn trajectory_hash(traj: &sim::state::Trajectory) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for snap in &traj.snapshots {
        mix(&snap.t.to_bits().to_le_bytes());
        for &c in &snap.int_state.counts {
            mix(&c.to_le_bytes());
        }
        for &v in &snap.real_state.values {
            mix(&v.to_bits().to_le_bytes());
        }
        for &f in &snap.flows.counts {
            mix(&f.to_le_bytes());
        }
    }
    h
}

/// Committed baselines: (model, backend) -> trajectory hash, captured on the dev
/// machine against the current compiler+runtime. Re-capture per the header.
const BASELINES: &[(&str, &str, u64)] = &[
    ("bimolecular", "gillespie", 0x54a38d360dcf4c01),
    ("bimolecular", "tau_leap", 0x9e1c4207f41b2750),
    ("bimolecular", "chain_binomial", 0x54a38d360dcf4c01),
    ("branching_si_symp_asym", "gillespie", 0x325b8b153b1b16d4),
    ("branching_si_symp_asym", "tau_leap", 0x0dfaa627e9055c84),
    ("branching_si_symp_asym", "chain_binomial", 0xae1bb55ced8410bd),
    ("malaria_two_species", "gillespie", 0x5ed03d7812021914),
    ("malaria_two_species", "tau_leap", 0x66287e80e10080a2),
    ("malaria_two_species", "chain_binomial", 0x132e1d7efc2da7d4),
    ("polio_age", "gillespie", 0x968a5308fde3affb),
    ("polio_age", "tau_leap", 0x606a247a0520fc63),
    ("polio_age", "chain_binomial", 0x7b8b1e77dbccab4b),
    ("polio_spatial_5", "gillespie", 0x5516309d3eedfda4),
    ("polio_spatial_5", "tau_leap", 0x11153b029262b757),
    ("polio_spatial_5", "chain_binomial", 0x3b8831126ad37aeb),
    ("ross_macdonald", "gillespie", 0xb8a901ca29312b3e),
    ("ross_macdonald", "tau_leap", 0x1ad985d8667cc3d7),
    ("ross_macdonald", "chain_binomial", 0xfaa942e09da2009a),
    ("seir_age", "gillespie", 0x42aa86e0753ea235),
    ("seir_age", "tau_leap", 0xdcacaf9c1489b282),
    ("seir_age", "chain_binomial", 0x1ea29e011a7eba67),
    ("seir_age_table_rates", "gillespie", 0xaefb0972f1798fc5),
    ("seir_age_table_rates", "tau_leap", 0x7ed4e557e6b34be5),
    ("seir_age_table_rates", "chain_binomial", 0x87d0504d39dc8044),
    ("seir_defines_adj", "gillespie", 0x6f777f70cb7742ca),
    ("seir_defines_adj", "tau_leap", 0x4192377b2c2f88ef),
    ("seir_defines_adj", "chain_binomial", 0xa443c47393008cf7),
    ("seir_defines_patch", "gillespie", 0xa7c867674ed33cf9),
    ("seir_defines_patch", "tau_leap", 0xa0eafb5336181c64),
    ("seir_defines_patch", "chain_binomial", 0x35818731f63a2b8b),
    ("seir_erlang", "gillespie", 0x9678d01f75671b6f),
    ("seir_erlang", "tau_leap", 0x87d4d188a2c5ac9e),
    ("seir_erlang", "chain_binomial", 0x08b695ddf690d3f0),
    ("seir_erlang_staged", "gillespie", 0xee741459747732f2),
    ("seir_erlang_staged", "tau_leap", 0x69a4c6829638bfc3),
    ("seir_erlang_staged", "chain_binomial", 0xd5463d6b91a7545d),
    ("seir_observations", "gillespie", 0x1512c82543641dbc),
    ("seir_observations", "tau_leap", 0x8905677ad6daa7df),
    ("seir_observations", "chain_binomial", 0x1620e4f54e9021bf),
    ("seir_seasonal_patch", "gillespie", 0xbab747d305e59679),
    ("seir_seasonal_patch", "tau_leap", 0x7888ce05381132b9),
    ("seir_seasonal_patch", "chain_binomial", 0x973dccbdeba49bb5),
    ("seir_spatial_5_inference", "tau_leap", 0x41a83f2151e11782),
    ("seir_spatial_5_inference", "chain_binomial", 0xfc6f6fe0c603429e),
    ("seir_vaccine", "gillespie", 0x17257cd9fa3ce428),
    ("seir_vaccine", "tau_leap", 0x1c86d86cfd23f9f7),
    ("seir_vaccine", "chain_binomial", 0xfb6b6f6bdba7e7d3),
    ("seir_vaccine_seasonal", "gillespie", 0xaadce0ddf1d680fd),
    ("seir_vaccine_seasonal", "tau_leap", 0xfc6ca3055b15b6fa),
    ("seir_vaccine_seasonal", "chain_binomial", 0xdce773319d7251e7),
    ("sia_anchored_dates", "gillespie", 0xa07df71463113b70),
    ("sia_anchored_dates", "tau_leap", 0xf6f676020e58593a),
    ("sia_anchored_dates", "chain_binomial", 0x5a594b3abc78f56b),
    ("sir_basic", "gillespie", 0xc58ddb854d12660a),
    ("sir_basic", "tau_leap", 0xf9ca085db3566082),
    ("sir_basic", "chain_binomial", 0x233d5bb24557cb84),
    ("sir_coupling", "gillespie", 0xfa90685fe7e20637),
    ("sir_coupling", "tau_leap", 0x70564a7912f9950a),
    ("sir_coupling", "chain_binomial", 0x909c2ae3a066dd5c),
    ("sir_demography", "gillespie", 0xf6238b4be3d98bcb),
    ("sir_demography", "tau_leap", 0xf5f7e09085d7889f),
    ("sir_demography", "chain_binomial", 0x57c2f3c4272fb8e0),
    ("sir_dim_annotated", "gillespie", 0xc8cc8178959c656f),
    ("sir_dim_annotated", "tau_leap", 0x2bdb7a0e695bff72),
    ("sir_dim_annotated", "chain_binomial", 0x3a9a794fc35272cd),
    ("sir_five_age", "gillespie", 0x0b027f6bf099d5ec),
    ("sir_five_age", "tau_leap", 0x84067824eba86471),
    ("sir_five_age", "chain_binomial", 0xd8287dd6daf58eb1),
    ("sir_init_table", "gillespie", 0xde69b188f2e80a65),
    ("sir_init_table", "tau_leap", 0x023e1e502b227977),
    ("sir_init_table", "chain_binomial", 0xfb5d74fc9bbb470e),
    ("sir_overdispersion", "tau_leap", 0x4c3d3790657de0e9),
    ("sir_overdispersion", "chain_binomial", 0x7be63a31759824b8),
    ("sir_patches_5", "gillespie", 0xbb5266c0c72c32b4),
    ("sir_patches_5", "tau_leap", 0x6104a200038c3b47),
    ("sir_patches_5", "chain_binomial", 0xb2a247f9ca9c2afe),
    ("sir_priors", "gillespie", 0xc58ddb854d12660a),
    ("sir_priors", "tau_leap", 0xf9ca085db3566082),
    ("sir_priors", "chain_binomial", 0x233d5bb24557cb84),
    ("sir_reservoir", "gillespie", 0x47bfd5ec6fefdb43),
    ("sir_reservoir", "tau_leap", 0xb128cf10d45b2056),
    ("sir_reservoir", "chain_binomial", 0x6f5c2c8af8307f5c),
    // Mixed int/real >=8-term aggregate (Fix-B trap #1 gate): a binding
    // extraction that reassociates the MixedPopSum fold order changes these.
    ("sir_reservoir_mixed", "gillespie", 0xa3b890243e0932a5),
    ("sir_reservoir_mixed", "tau_leap", 0xccb85e8f9b693a1e),
    ("sir_reservoir_mixed", "chain_binomial", 0x0bacf4e75cfcb7fc),
    ("sir_spatial_sum", "gillespie", 0x65d363618fc40fb4),
    ("sir_spatial_sum", "tau_leap", 0xd38ed3b3bfe9c9fa),
    ("sir_spatial_sum", "chain_binomial", 0xd38ed3b3bfe9c9fa),
    // overdispersion model: gillespie/ode capability-skip, so tau-leap + chain-binomial only
    ("sir_two_overdispersed", "tau_leap", 0x5af05162d3c983bd),
    ("sir_two_overdispersed", "chain_binomial", 0x47b4ab5edd2fb5c4),
    ("sir_two_patch", "gillespie", 0xe9f432f7882e9b70),
    ("sir_two_patch", "tau_leap", 0xc432b4955e374ed6),
    ("sir_two_patch", "chain_binomial", 0xa1c9f945649cc4fa),
    ("sirv_anchored_calendar", "gillespie", 0xec592cdf358a308e),
    ("sirv_anchored_calendar", "tau_leap", 0xdf8d3f99b0d42cab),
    ("sirv_anchored_calendar", "chain_binomial", 0x557cef37b9b035b1),
    // ODE backend (deterministic; added per the four-backend landing
    // condition). Captured against the post-Fix-B compiler/runtime.
    ("bimolecular", "ode", 0x9b78f38544509e31),
    ("branching_si_symp_asym", "ode", 0x49c0178b07ff5a11),
    ("malaria_two_species", "ode", 0xa196af484eabfada),
    ("polio_age", "ode", 0x8666c2e727620de6),
    ("polio_spatial_5", "ode", 0x81ed3b07bb11e95a),
    ("ross_macdonald", "ode", 0xabf137964976a29a),
    ("seir_age", "ode", 0xe751bceb05d6d96f),
    ("seir_age_table_rates", "ode", 0x939b7744fa70ca36),
    ("seir_defines_adj", "ode", 0x572dd8daf4bf4a11),
    ("seir_defines_patch", "ode", 0x673d152ffc0d0062),
    ("seir_erlang", "ode", 0x9b29cf1f075449e3),
    ("seir_erlang_staged", "ode", 0x0c24c269bfbacab6),
    ("seir_observations", "ode", 0x7a508de2aa682947),
    ("seir_seasonal_patch", "ode", 0x4c239067cbb27691),
    ("seir_vaccine", "ode", 0xe4e6cdeb29305f53),
    ("seir_vaccine_seasonal", "ode", 0x8e29acb2da8ea04d),
    ("sia_anchored_dates", "ode", 0x9b58a5d46cc8deb2),
    ("sir_basic", "ode", 0xb2fa9101b5997ef6),
    ("sir_coupling", "ode", 0xdaa46bf899ced8ec),
    ("sir_demography", "ode", 0xaace1bf8fd9af151),
    ("sir_dim_annotated", "ode", 0xd4eabe748e47ff65),
    ("sir_five_age", "ode", 0x512ca373f1110261),
    ("sir_init_table", "ode", 0x635f90433ba03360),
    ("sir_patches_5", "ode", 0xef155405de7c8672),
    ("sir_priors", "ode", 0xb2fa9101b5997ef6),
    ("sir_reservoir", "ode", 0xfc216d83dd10cc5f),
    ("sir_reservoir_mixed", "ode", 0x9f6524558df9a394),
    ("sir_spatial_sum", "ode", 0x4a4e770716cd1bad),
    ("sir_two_patch", "ode", 0xdf3aace63920506c),
    ("sirv_anchored_calendar", "ode", 0xc72bec34c422826d),
];

#[test]
fn gate_golden_trajectories_are_byte_identical() {
    // Match smoke_all_golden: legacy degenerate-rate mode + no interventions, so
    // the baseline behavior is well-defined for the whole corpus.
    sim::eval_stats::set_allow_degenerate_rates(true);
    let capture = std::env::var("CAMDL_CAPTURE_BASELINE").is_ok();

    let models = discover_models();
    assert!(!models.is_empty(), "no *.ir.json in ocaml/golden/");

    let lookup = |name: &str, backend: &str| -> Option<u64> {
        BASELINES.iter()
            .find(|(n, b, _)| *n == name && *b == backend)
            .map(|(_, _, h)| *h)
    };

    let mut captured: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for name in &models {
        let mut model = load_and_apply_baseline(name);
        model.interventions.clear();
        let compiled = match CompiledModel::new(model.clone()) {
            Ok(c) => c,
            Err(_) => continue, // models that don't compile under baseline are out of scope here
        };
        let params = compiled.default_params.clone();
        let t_start = model.simulation.t_start;
        let t_end = model.simulation.t_end.min(30.0);

        // ODE is included to satisfy the Fix-B/D landing condition ("byte-
        // identical under all four backends"): the proposal requires ODE
        // coverage, but the gate previously pinned only the three stochastic
        // backends. ODE is deterministic (seed ignored); capability-skip drops
        // models it can't run (e.g. overdispersion), and the run-error
        // `continue` below drops any it errors on.
        let backends: &[(&str, SimConfig)] = &[
            ("gillespie", SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None })),
            ("tau_leap", SimConfig::TauLeap(TauLeapConfig { t_start, t_end, dt: 0.5 })),
            ("chain_binomial", SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: 1.0 })),
            ("ode", SimConfig::Ode(OdeConfig { t_start, t_end, dt: 1.0 })),
        ];
        let required = compiled.required_capabilities();
        for (backend, config) in backends {
            let sim: &dyn Simulate = match *backend {
                "gillespie" => &GillespieSim,
                "tau_leap" => &TauLeapSim,
                "ode" => &OdeSim,
                _ => &ChainBinomialSim,
            };
            if !(required - sim.capabilities()).is_empty() {
                continue;
            }
            let traj = match sim.run(&compiled, &params, SEED, config) {
                Ok(t) => t,
                Err(_) => continue, // baseline-time sim errors are not this gate's concern
            };
            let hash = trajectory_hash(&traj);
            if capture {
                captured.push(format!("    (\"{name}\", \"{backend}\", 0x{hash:016x}),"));
            } else {
                match lookup(name, backend) {
                    Some(expected) => assert_eq!(
                        hash, expected,
                        "TRAJECTORY CHANGED for {name}/{backend}: a refactor perturbed \
                         the trajectory (got 0x{hash:016x}, expected 0x{expected:016x})"
                    ),
                    None => missing.push(format!("{name}/{backend}")),
                }
            }
        }
    }

    if capture {
        eprintln!("\n// <<CAPTURED-BASELINES>> — paste into BASELINES:");
        for line in &captured {
            eprintln!("{line}");
        }
        eprintln!("// ({} entries)\n", captured.len());
    } else {
        assert!(
            missing.is_empty(),
            "no baseline for: {missing:?} — run with CAMDL_CAPTURE_BASELINE=1 and paste the table"
        );
    }
}
