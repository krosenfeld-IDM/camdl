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

/// A 2-patch SIR whose population table is loaded via `read("pop.tsv")` at
/// compile time and baked into the IR. Editing `pop.tsv` (without touching the
/// `.camdl`) must invalidate the cache (gh#260).
const SIR_PATCHES: &str = r#"
time_unit = 'days
dimensions { patch = [north, south] }
compartments { S, I, R }
stratify(by = patch)
tables {
  N0 : patch = read("pop.tsv")
}
parameters {
  beta  : rate        in [0.001, 1.0]
  gamma : rate        in [0.01,  1.0]
  I0    : count       in [1, 100]
}
let N[p in patch] = S[p] + I[p] + R[p]
transitions {
  infection[p in patch] : S[p] --> I[p]  @ beta * S[p] * I[p] / N[p]
  recovery[p in patch]  : I[p] --> R[p]  @ gamma * I[p]
}
init {
  S[p in patch] = N0[p] - I0
  I[p in patch] = I0
}
simulate { from = 0 'days  to = 28 'days }
"#;

const POP_TSV_A: &str = "patch\tN0\nnorth\t50000\nsouth\t30000\n";
const POP_TSV_B: &str = "patch\tN0\nnorth\t99999\nsouth\t30000\n";

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

/// Run `simulate` on the 2-patch model (params differ from `SIR`; `N0` is a
/// read() table, not a CLI param).
fn run_patches(bin: &Path, shim_dir: &Path, model: &Path, cache_dir: &Path, out: &Path) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let st = Command::new(bin)
        .args([
            "simulate", model.to_str().unwrap(),
            "--backend", "chain_binomial", "--seed", "1",
            "--param", "beta=0.3", "--param", "gamma=0.1", "--param", "I0=5",
            "--output-dir", out.to_str().unwrap(), "--progress", "none",
        ])
        .env("PATH", format!("{}:{}", shim_dir.display(), old_path))
        .env("CAMDL_IR_CACHE_DIR", cache_dir)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .status().expect("spawn");
    assert!(st.success(), "simulate (patches) should succeed");
}

/// gh#260: a file loaded via `read()` is a compile input. Editing `pop.tsv`
/// without touching the `.camdl` must invalidate the cache and recompile —
/// otherwise camdl silently serves IR built from the stale populations.
#[test]
fn editing_a_read_loaded_file_invalidates_the_cache() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("patches.camdl");
    std::fs::write(&model, SIR_PATCHES).unwrap();
    let pop = tmp.path().join("pop.tsv");
    std::fs::write(&pop, POP_TSV_A).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    let cache = tmp.path().join("ircache");

    run_patches(&bin, &shim, &model, &cache, &tmp.path().join("o1"));
    assert_eq!(compiles(&counter), 1, "first run compiles once (cache miss)");

    run_patches(&bin, &shim, &model, &cache, &tmp.path().join("o2"));
    assert_eq!(compiles(&counter), 1, "unchanged read() file → cache hit, no recompile");

    // Edit the read()-loaded table; the .camdl is byte-identical.
    std::fs::write(&pop, POP_TSV_B).unwrap();

    run_patches(&bin, &shim, &model, &cache, &tmp.path().join("o3"));
    assert_eq!(compiles(&counter), 2,
        "editing the read()-loaded pop.tsv must invalidate the cache and recompile");

    run_patches(&bin, &shim, &model, &cache, &tmp.path().join("o4"));
    assert_eq!(compiles(&counter), 2,
        "re-run after the edit hits the cache (now keyed to the new pop.tsv)");
}

/// gh#260: correctness under the GLOBAL shared cache. Two byte-identical
/// `.camdl` files (→ identical cache key) in different directories with
/// DIFFERENT `pop.tsv` contents must not share an IR entry — the read()-inputs
/// are re-resolved against the *current* model's directory and re-hashed, so B
/// recompiles rather than serving A's IR built from A's populations.
#[test]
fn same_model_different_read_data_do_not_share_a_cache_entry() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let shared_cache = tmp.path().join("ircache"); // ONE global cache for both
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);

    // Context A: model + popA.
    let dir_a = tmp.path().join("a");
    std::fs::create_dir_all(&dir_a).unwrap();
    let model_a = dir_a.join("patches.camdl");
    std::fs::write(&model_a, SIR_PATCHES).unwrap();
    std::fs::write(dir_a.join("pop.tsv"), POP_TSV_A).unwrap();

    // Context B: BYTE-IDENTICAL model, a DIFFERENT pop.tsv.
    let dir_b = tmp.path().join("b");
    std::fs::create_dir_all(&dir_b).unwrap();
    let model_b = dir_b.join("patches.camdl");
    std::fs::write(&model_b, SIR_PATCHES).unwrap();
    std::fs::write(dir_b.join("pop.tsv"), POP_TSV_B).unwrap();

    run_patches(&bin, &shim, &model_a, &shared_cache, &tmp.path().join("oa"));
    assert_eq!(compiles(&counter), 1, "context A compiles once (cold cache)");

    run_patches(&bin, &shim, &model_b, &shared_cache, &tmp.path().join("ob"));
    assert_eq!(compiles(&counter), 2,
        "same model bytes but different pop.tsv → must recompile, not reuse A's IR");
}

/// Like `run_simulate` but with `CAMDL_NO_CONSTANT_FOLD` set, so camdlc emits
/// the unfolded IR — a different compile output for the same model.
fn run_simulate_fold_off(bin: &Path, shim_dir: &Path, model: &Path, cache_dir: &Path, out: &Path) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let st = Command::new(bin)
        .args([
            "simulate", model.to_str().unwrap(),
            "--backend", "chain_binomial", "--seed", "1",
            "--param", "beta=0.3", "--param", "gamma=0.1", "--param", "N0=1000",
            "--output-dir", out.to_str().unwrap(), "--progress", "none",
        ])
        .env("PATH", format!("{}:{}", shim_dir.display(), old_path))
        .env("CAMDL_IR_CACHE_DIR", cache_dir)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .env("CAMDL_NO_CONSTANT_FOLD", "1")
        .status().expect("spawn");
    assert!(st.success(), "simulate (fold off) should succeed");
}

#[test]
fn toggling_constant_fold_invalidates_the_cache() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("sir.camdl");
    std::fs::write(&model, SIR).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    let cache = tmp.path().join("ircache");

    // Fold ON (the default): compile + cache.
    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o1"), false);
    assert_eq!(compiles(&counter), 1, "fold-on: first run compiles once");

    // Same model, but CAMDL_NO_CONSTANT_FOLD now set → camdlc emits a DIFFERENT
    // (unfolded) IR. The flag is in the cache key, so this must recompile, not
    // serve the folded variant.
    run_simulate_fold_off(&bin, &shim, &model, &cache, &tmp.path().join("o2"));
    assert_eq!(compiles(&counter), 2, "toggling CAMDL_NO_CONSTANT_FOLD recompiles (flag in key)");

    // The two variants are SEPARATE entries, not an overwrite: fold-off reuses
    // its own entry, and fold-on still hits its original one.
    run_simulate_fold_off(&bin, &shim, &model, &cache, &tmp.path().join("o3"));
    assert_eq!(compiles(&counter), 2, "fold-off reuses its own cache entry");
    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("o4"), false);
    assert_eq!(compiles(&counter), 2, "fold-on still hits its original entry");
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

/// Spawn `simulate <model>` WITHOUT waiting — for the concurrency test. The
/// returned `Child` is `wait()`ed by the caller after all peers are launched,
/// so the processes genuinely race on the cold cache.
fn spawn_simulate(bin: &Path, shim_dir: &Path, model: &Path, cache_dir: &Path, out: &Path, seed: &str)
    -> std::process::Child
{
    let old_path = std::env::var("PATH").unwrap_or_default();
    Command::new(bin)
        .args([
            "simulate", model.to_str().unwrap(),
            "--backend", "chain_binomial", "--seed", seed,
            "--param", "beta=0.3", "--param", "gamma=0.1", "--param", "N0=1000",
            "--output-dir", out.to_str().unwrap(), "--progress", "none",
        ])
        .env("PATH", format!("{}:{}", shim_dir.display(), old_path))
        .env("CAMDL_IR_CACHE_DIR", cache_dir)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        // Discard the TSV so a full pipe buffer can't stall a worker.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn().expect("spawn")
}

/// gh#214: N concurrent `simulate` of the SAME model on a COLD cache must
/// compile camdlc exactly ONCE (single-flight), not N times. Before the fix
/// every worker missed the cache and spawned its own ~11 GB camdlc → OOM storm;
/// the reproduction counted N invocations. With the single-flight lock one
/// worker compiles and publishes the IR while the rest wait and serve it.
///
/// The assertion is on the camdlc *invocation count* (via the counting shim),
/// which is timing-independent — it does not depend on how the N processes
/// interleave, only that they all contend on one cold key.
#[test]
fn concurrent_simulate_compiles_camdlc_once() {
    let Some((bin, real)) = skip_if_unbuilt() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("sir.camdl");
    std::fs::write(&model, SIR).unwrap();
    let counter = tmp.path().join("compiles.log");
    let shim = counting_shim(tmp.path(), &real, &counter);
    // COLD cache: a dir that does not yet exist — every worker misses.
    let cache = tmp.path().join("ircache");

    const N: usize = 6;
    let children: Vec<_> = (0..N)
        .map(|i| spawn_simulate(
            &bin, &shim, &model, &cache,
            &tmp.path().join(format!("w{i}")), &i.to_string()))
        .collect();

    for (i, mut c) in children.into_iter().enumerate() {
        let st = c.wait().expect("wait");
        assert!(st.success(), "worker {i} should succeed (exit 0)");
    }

    // The single-flight lock dedupes the compile: exactly one camdlc run.
    assert_eq!(
        compiles(&counter), 1,
        "{N} concurrent cold-cache simulates must compile camdlc ONCE (single-flight), \
         not once-per-worker (the gh#214 storm)");

    // The lock file must not linger after the leader finished — it is removed
    // on guard drop so the entry is a clean cache hit thereafter.
    let leftover_locks: Vec<_> = std::fs::read_dir(&cache).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
        .collect();
    assert!(leftover_locks.is_empty(),
        "single-flight .lock must be removed after compile, found: {leftover_locks:?}");

    // A subsequent run is a pure cache hit: no new compile.
    run_simulate(&bin, &shim, &model, &cache, &tmp.path().join("after"), false);
    assert_eq!(compiles(&counter), 1,
        "a warm run after the concurrent storm must hit the cache (0 new compiles)");
}
