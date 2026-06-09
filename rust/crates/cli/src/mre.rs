//! `camdl mre` — minimal-reproducible-example bundles (gh#212).
//!
//! Packages the full input closure of a fit (the model, the model's
//! compile-time `read()` files, observed data, fixed params) into a single
//! `.tar.gz` so a bug report reproduces from one file. See
//! `docs/dev/proposals/2026-06-09-mre-bundle.md`.
//!
//! v1 scope: the pack side for `mre fit`, common closure. Self-containment is
//! enforced by a "contained relative layout" rule — every input must live under
//! the fit.toml's directory, so copying preserves each file's relative path and
//! the bundled fit.toml needs no rewriting. Absolute / `../`-escaping paths are
//! a portability smell (gh#211) and hard-error. Upstream-artifact seeds
//! (`init = survey_top_k` / `from_mle` / `from_posterior` / `from_params`) are
//! not yet bundled and hard-error with guidance.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::args::{MreFitArgs, MreSimulateArgs};
use crate::fit::config_v2::{DataSpec, FitConfigV2};

const SCHEMA_VERSION: u32 = 1;

// ── camdlc --emit-deps sidecar ───────────────────────────────────────────────

#[derive(Deserialize)]
struct DepFile {
    #[serde(default)]
    reads: Vec<DepEntry>,
}
#[derive(Deserialize)]
struct DepEntry {
    #[allow(dead_code)]
    as_written: String,
    resolved: String,
}

// ── manifest ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MreManifest {
    schema_version: u32,
    kind: String,          // "fit"
    reproduce: String,     // the exact command the maintainer runs
    camdl_version: String, // the binary that packed it
    data_included: bool,
    inputs: Vec<BundledInput>,
}

#[derive(Serialize, Clone)]
struct BundledInput {
    role: String,
    /// Bundle-relative destination path.
    dest: String,
    bytes: u64,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u64>,
}

// ── a file to bundle, before it is copied ────────────────────────────────────

struct InputRef {
    role: &'static str,
    /// Absolute source path on disk.
    src: PathBuf,
    /// Bundle-relative destination (== path relative to the project root).
    dest: String,
    /// Observed-data file → eligible for the consent banner + row count.
    is_data: bool,
}

// ── entry points ─────────────────────────────────────────────────────────────

pub(crate) fn cmd_mre_fit(args: &MreFitArgs) {
    if let Err(e) = run_fit(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

pub(crate) fn cmd_mre_simulate(_args: &MreSimulateArgs) {
    eprintln!(
        "error: `camdl mre simulate` is not implemented yet (gh#212).\n  \
         `camdl mre fit <fit.toml>` is available now; the simulate bundler \
         lands in the next increment."
    );
    std::process::exit(1);
}

// ── fit bundler ──────────────────────────────────────────────────────────────

fn run_fit(args: &MreFitArgs) -> Result<(), String> {
    let config = args.config.as_path();
    if !config.exists() {
        return Err(format!("fit config not found: {}", config.display()));
    }
    // The project root: the fit.toml's directory. Every input must live under
    // it (the contained-relative-layout rule).
    let root = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let cfg_text = fs::read_to_string(config)
        .map_err(|e| format!("cannot read {}: {e}", config.display()))?;
    let cfg: FitConfigV2 = toml::from_str(&cfg_text)
        .map_err(|e| format!("cannot parse {} as a fit.toml: {e}", config.display()))?;

    check_supported(&cfg)?;

    // ── collect the input closure ──
    let mut inputs: Vec<InputRef> = Vec::new();

    // Model + its compile-time read() closure (asked of the compiler).
    let model_path = root.join(&cfg.model.camdl);
    if !model_path.exists() {
        return Err(format!("model not found: {} (from [model] camdl = \"{}\")",
            model_path.display(), cfg.model.camdl));
    }
    inputs.push(input_ref("model", &model_path, &root)?);

    let deps = read_closure(&model_path)?;
    for resolved in deps {
        inputs.push(input_ref("read_closure", Path::new(&resolved), &root)?);
    }

    // Observed data (unless --no-data) + fixed params + synthetic truth.
    let include_data = !args.no_data;
    if let Some(ds) = &cfg.data {
        if include_data {
            for rel in data_files(ds) {
                inputs.push(input_ref_data(&root.join(&rel), &root)?);
            }
        }
    }
    if let Some(ff) = &cfg.fixed.from_file {
        inputs.push(input_ref("fixed_params", &root.join(ff), &root)?);
    }
    if let Some(sy) = &cfg.synthetic {
        inputs.push(input_ref("true_params", &root.join(&sy.true_params), &root)?);
    }

    // The fit.toml itself, at the bundle root.
    let config_dest = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fit.toml".to_string());

    // Dedup by destination (a read() file could coincide with another input).
    inputs.sort_by(|a, b| a.dest.cmp(&b.dest));
    inputs.dedup_by(|a, b| a.dest == b.dest);

    // ── stage → manifest → README → tarball ──
    let staging = tempfile::tempdir()
        .map_err(|e| format!("cannot create staging dir: {e}"))?;
    let stage_root = staging.path();

    // Copy the (possibly path-contained) inputs, recording the inventory.
    let mut manifest_inputs: Vec<BundledInput> = Vec::new();
    let mut data_banner: Vec<(String, u64)> = Vec::new();
    for inp in &inputs {
        let dst = stage_root.join(&inp.dest);
        copy_into(&inp.src, &dst)?;
        let (bytes, sha) = digest_file(&dst)?;
        let rows = if inp.is_data { Some(count_data_rows(&dst)?) } else { None };
        if inp.is_data {
            data_banner.push((inp.dest.clone(), rows.unwrap_or(0)));
        }
        manifest_inputs.push(BundledInput {
            role: inp.role.to_string(),
            dest: inp.dest.clone(),
            bytes,
            sha256: sha,
            rows,
        });
    }
    // The fit.toml (copied verbatim — all input paths are contained-relative,
    // so they resolve unchanged inside the bundle).
    copy_into(config, &stage_root.join(&config_dest))?;
    let (cfg_bytes, cfg_sha) = digest_file(&stage_root.join(&config_dest))?;
    manifest_inputs.push(BundledInput {
        role: "fit_config".to_string(),
        dest: config_dest.clone(),
        bytes: cfg_bytes,
        sha256: cfg_sha,
        rows: None,
    });

    let reproduce = format!("camdl fit run {config_dest}");
    let manifest = MreManifest {
        schema_version: SCHEMA_VERSION,
        kind: "fit".to_string(),
        reproduce: reproduce.clone(),
        camdl_version: crate::version::VERSION_SHORT.to_string(),
        data_included: include_data && !data_banner.is_empty(),
        inputs: manifest_inputs,
    };
    let manifest_toml = toml::to_string_pretty(&manifest)
        .map_err(|e| format!("cannot serialize manifest: {e}"))?;
    fs::write(stage_root.join("manifest.toml"), manifest_toml)
        .map_err(|e| format!("cannot write manifest: {e}"))?;
    fs::write(stage_root.join("README.md"), readme(&reproduce, include_data))
        .map_err(|e| format!("cannot write README: {e}"))?;

    // ── tarball ──
    let out = args.bundle.clone().unwrap_or_else(|| {
        let stem = config
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "fit".to_string());
        PathBuf::from(format!("{stem}.mre.tar.gz"))
    });
    let bundle_name = out
        .file_name()
        .map(|n| n.to_string_lossy().trim_end_matches(".tar.gz").to_string())
        .unwrap_or_else(|| "mre".to_string());
    write_tarball(&out, &bundle_name, stage_root)?;

    // ── report + consent banner ──
    if !data_banner.is_empty() {
        let listed = data_banner
            .iter()
            .map(|(name, rows)| format!("{name} ({rows} rows)"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("\u{26a0}  This bundle contains observed data: {listed}.");
        eprintln!("   Share only with the maintainer. (Use --no-data for a structure-only bundle.)");
    }
    println!("\u{2713} wrote {} ({} files)", out.display(), inputs.len() + 1);
    println!("  reproduce: {reproduce}");
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Compile the model and parse its read-closure depfile → resolved paths.
fn read_closure(model_path: &Path) -> Result<Vec<String>, String> {
    let tmp = tempfile::Builder::new()
        .suffix(".deps.json")
        .tempfile()
        .map_err(|e| format!("cannot create depfile: {e}"))?;
    let model_str = model_path
        .to_str()
        .ok_or_else(|| format!("non-UTF8 model path: {}", model_path.display()))?;
    crate::util::camdlc_emit_deps(model_str, tmp.path())
        .map_err(|e| format!("compiling {} failed:\n{e}", model_path.display()))?;
    let text = fs::read_to_string(tmp.path())
        .map_err(|e| format!("cannot read depfile: {e}"))?;
    let df: DepFile = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse depfile: {e}"))?;
    Ok(df.reads.into_iter().map(|e| e.resolved).collect())
}

/// Refuse fits whose init seeds from an upstream artifact mre cannot yet bundle.
fn check_supported(cfg: &FitConfigV2) -> Result<(), String> {
    for (name, stage) in &cfg.stages {
        let v = toml::Value::try_from(stage)
            .map_err(|e| format!("internal: cannot inspect stage `{name}`: {e}"))?;
        let Some(t) = v.as_table() else { continue };
        match t.get("init") {
            // Unit variant: only `survey_top_k` is an upstream seed.
            Some(toml::Value::String(s)) if s == "survey_top_k" => {
                return Err(unsupported_seed(name, "init = \"survey_top_k\" (seeds from a survey landscape)"));
            }
            // Struct variants `from_mle` / `from_posterior` / `from_params`
            // serialize as a single-key table.
            Some(toml::Value::Table(it)) => {
                let which = it.keys().next().map(String::as_str).unwrap_or("?");
                return Err(unsupported_seed(
                    name,
                    &format!("init = {{ {which} }} (seeds from an external file or fit dir)"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn unsupported_seed(stage: &str, what: &str) -> String {
    format!(
        "`camdl mre` does not yet bundle fits whose init seeds from an upstream \
         artifact: stage `{stage}` uses {what}.\n  \
         Make the fit self-contained (init = \"lhs\" / \"single\" / \"from_prior\") \
         or wait for a later mre version that bundles seed artifacts."
    )
}

/// Data file paths (relative to the project root) named by a `[data]` spec.
fn data_files(ds: &DataSpec) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(f) = &ds.file {
        out.push(f.clone());
    }
    for v in ds.observations.values() {
        out.push(v.clone());
    }
    if let Some(h) = &ds.holdout {
        for v in h.values() {
            out.push(v.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn input_ref(role: &'static str, file: &Path, root: &Path) -> Result<InputRef, String> {
    Ok(InputRef { role, src: file.to_path_buf(), dest: rel_to_root(root, file)?, is_data: false })
}
fn input_ref_data(file: &Path, root: &Path) -> Result<InputRef, String> {
    Ok(InputRef { role: "data", src: file.to_path_buf(), dest: rel_to_root(root, file)?, is_data: true })
}

/// A bundled input's destination = its path relative to the project root.
/// Errors if the file is absolute or escapes the root (`../`) — a portability
/// smell (gh#211) that breaks relocation.
fn rel_to_root(root: &Path, file: &Path) -> Result<String, String> {
    let rc = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve project dir {}: {e}", root.display()))?;
    let fc = file
        .canonicalize()
        .map_err(|e| format!("input not found: {} ({e})", file.display()))?;
    fc.strip_prefix(&rc)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            format!(
                "input escapes the project directory: {}\n  \
                 `camdl mre` requires every input to live under the fit.toml's \
                 directory ({}). Absolute or `../` paths aren't portable (gh#211) \
                 — move the file under the project, or make its path relative.",
                file.display(),
                rc.display()
            )
        })
}

fn copy_into(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} → bundle: {e}", src.display()))
}

fn digest_file(path: &Path) -> Result<(u64, String), String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok((bytes.len() as u64, hex::encode(h.finalize())))
}

/// Count non-empty, non-comment data rows (minus the header) for the inventory.
fn count_data_rows(path: &Path) -> Result<u64, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut rows: u64 = 0;
    let mut seen_header = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if !seen_header {
            seen_header = true; // first real line is the header
            continue;
        }
        rows += 1;
    }
    Ok(rows)
}

fn write_tarball(out: &Path, bundle_name: &str, stage_root: &Path) -> Result<(), String> {
    let f = fs::File::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all(bundle_name, stage_root)
        .map_err(|e| format!("cannot write tarball: {e}"))?;
    let enc = tar.into_inner().map_err(|e| format!("cannot finalize tar: {e}"))?;
    enc.finish().map_err(|e| format!("cannot finish gzip: {e}"))?;
    Ok(())
}

fn readme(reproduce: &str, data_included: bool) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# camdl MRE bundle\n");
    let _ = writeln!(s, "Minimal reproducible example for a `camdl` bug report.\n");
    let _ = writeln!(s, "## Reproduce\n\n```\n{reproduce}\n```\n");
    let _ = writeln!(s, "All paths are bundle-relative; run from this directory after unpacking.\n");
    if data_included {
        let _ = writeln!(s, "> This bundle includes observed data. Handle per your data-sharing policy.\n");
    } else {
        let _ = writeln!(s, "> Structure-only bundle (no observed data values).\n");
    }
    let _ = writeln!(s, "See `manifest.toml` for the full input inventory.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
[model]
camdl = "m.camdl"
[estimate.beta]
[fixed]
gamma = 0.1
[stages.fit]
algorithm = "if2"
backend = "chain_binomial"
chains = 2
particles = 100
iterations = 5
cooling = 0.7
"#;

    fn cfg_from(s: &str) -> FitConfigV2 {
        toml::from_str(s).expect("parse fit.toml")
    }

    #[test]
    fn supported_default_init_is_ok() {
        assert!(check_supported(&cfg_from(BASE)).is_ok());
    }

    #[test]
    fn survey_top_k_seed_is_rejected() {
        let s = format!("{BASE}init = \"survey_top_k\"\n");
        let err = check_supported(&cfg_from(&s)).unwrap_err();
        assert!(err.contains("survey_top_k"), "expected guidance naming the seed: {err}");
    }

    #[test]
    fn data_files_collects_file_and_holdout() {
        let s = format!(
            "{BASE}[data]\nfile = \"cases.tsv\"\n[data.holdout]\ncases = \"holdout.tsv\"\n"
        );
        let cfg = cfg_from(&s);
        let files = data_files(cfg.data.as_ref().unwrap());
        assert_eq!(files, vec!["cases.tsv".to_string(), "holdout.tsv".to_string()]);
    }

    #[test]
    fn rel_to_root_contains_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("data");
        std::fs::create_dir_all(&sub).unwrap();
        let f = sub.join("x.tsv");
        std::fs::write(&f, b"x").unwrap();
        // contained → relative dest, forward slashes
        assert_eq!(rel_to_root(root, &f).unwrap(), "data/x.tsv");
        // a file outside the root (absolute, escaping) → hard error
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(rel_to_root(root, outside.path()).is_err());
    }
}
