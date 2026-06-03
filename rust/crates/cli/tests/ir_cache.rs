//! The compiled-IR cache: a `.camdl` compiled once is reused across separate
//! `camdl` invocations (keyed on the model content + the camdlc/IR version),
//! so camdlc is skipped on a cache hit. Verified with a counting camdlc shim
//! on PATH and a per-test `CAMDL_IR_CACHE_DIR` (so the real user cache is
//! never touched). Editing the model, or `--no-ir-cache`, must recompile.

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl")
}

fn real_camdlc() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ocaml/_build/default/bin/camdlc.exe")
}

fn skip_if_unbuilt() -> Option<(PathBuf, PathBuf)> {
    let bin = camdl_bin();
    let cc = real_camdlc();
    if !bin.exists() || !cc.exists() {
        eprintln!("skipping: camdl/camdlc not built");
        return None;
    }
    Some((bin, cc))
}

const SIR: &str = r#"
time_unit = 'days
compartments { S, I, R }
parameters {
  beta  : rate  in [0.001, 5.0]
  gamma : rate  in [0.01, 1.0]
  N0    : count in [100, 10000]
}
let N = S + I + R
transitions {
  infection : S --> I @ beta * S * I / N
  recovery  : I --> R @ gamma * I
}
init { S = 499  I = 1 }
simulate { from = 0 'days  to = 20 'days }
"#;

/// A camdlc wrapper that appends a line per *compile* (not the
/// `--camdl-version` probe) before exec'ing the real camdlc.
fn counting_shim(dir: &Path, real: &Path, counter: &Path) -> PathBuf {
    let shim = dir.join("camdlc");
    std::fs::write(&shim, format!(
        "#!/bin/sh\ncase \"$1\" in\n  --camdl-version) ;;\n  *) echo x >> '{}' ;;\nesac\nexec '{}' \"$@\"\n",
        counter.display(), real.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&shim).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&shim, p).unwrap();
    }
    dir.to_path_buf()
}

fn compiles(counter: &Path) -> usize {
    std::fs::read_to_string(counter).map(|s| s.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0)
}

/// Run `simulate <model>` once with the shim ahead on PATH and the cache dir
/// pointed at `cache_dir`. `no_cache` adds `--no-ir-cache`.
fn run_simulate(bin: &Path, shim_dir: &Path, model: &Path, cache_dir: &Path, out: &Path, no_cache: bool) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut args: Vec<&str> = vec![
        "simulate", model.to_str().unwrap(),
        "--backend", "chain_binomial", "--seed", "1",
        "--param", "beta=0.3", "--param", "gamma=0.1", "--param", "N0=1000",
        "--output-dir", out.to_str().unwrap(), "--progress", "none",
    ];
    if no_cache { args.push("--no-ir-cache"); }
    let st = Command::new(bin)
        .args(&args)
        .env("PATH", format!("{}:{}", shim_dir.display(), old_path))
        .env("CAMDL_IR_CACHE_DIR", cache_dir)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .status().expect("spawn");
    assert!(st.success(), "simulate should succeed");
}

#[test]
fn ir_compiled_once_then_reused_across_runs() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("sir.camdl");
    std::fs::write(&model, SIR).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    let cache = tmp.path().join("ircache");

    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o1"), false);
    assert_eq!(compiles(&counter), 1, "first run compiles once (cache miss)");

    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o2"), false);
    assert_eq!(compiles(&counter), 1,
        "second run of the SAME model reuses the cached IR — camdlc must NOT run again");
}

#[test]
fn editing_the_model_invalidates_the_cache() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("sir.camdl");
    std::fs::write(&model, SIR).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    let cache = tmp.path().join("ircache");

    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o1"), false);
    assert_eq!(compiles(&counter), 1);

    // Change the model content (longer horizon) → different key → recompile.
    std::fs::write(&model, SIR.replace("to = 20 'days", "to = 40 'days")).unwrap();
    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o2"), false);
    assert_eq!(compiles(&counter), 2, "an edited model must recompile (content is in the key)");
}

#[test]
fn no_ir_cache_flag_bypasses_the_cache() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("sir.camdl");
    std::fs::write(&model, SIR).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    let cache = tmp.path().join("ircache");

    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o1"), true);
    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o2"), true);
    assert_eq!(compiles(&counter), 2, "--no-ir-cache must recompile every run");
}
