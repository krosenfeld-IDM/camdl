//! Typed CAS inputs — the unified abstraction for content-addressed runs.
//!
//! Every CAS-emitting subcommand (`profile`, `simulate --cas`,
//! `batch run`, `fit run`) implements [`CasInputs`] for its
//! single-realization input set. The trait fixes how a run is hashed
//! and where it lands on disk, so the four commands can't drift on
//! canonical-string conventions or layout decisions.
//!
//! ## Four roles every input plays
//!
//! - **Content** (in hash, determines validity): model IR bytes, data
//!   bytes, algorithm hyperparams, seed for stochastic methods,
//!   `starts_from` upstream lineage.
//! - **Path** (in path, determines readability): the 8-char hash
//!   prefix plus a human stem.
//! - **Replicate** (parent-child relationship): inputs that *vary*
//!   an otherwise-identical run for sensitivity analysis (e.g. `seed`
//!   across a multi-seed sweep).
//! - **Ephemeral** (nowhere): `--parallel`, progress mode, output
//!   mirror paths. Recorded in `argv` for forensics, not in any hash.
//!
//! See `docs/dev/proposals/2026-04-28-cas-typed-runs-and-profile-stages.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::run_meta::{Run, RunKind, RunStatus};

// ─── ContentHash ─────────────────────────────────────────────────────────────

/// 64-char hex SHA-256 of canonicalized content inputs. Newtype so the
/// type system distinguishes content hashes from arbitrary strings (a
/// `String` parameter that happens to hold a hash will not type-check).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(String);

impl ContentHash {
    /// Hex over arbitrary bytes — the standard way to construct a
    /// ContentHash from canonicalized input.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex_of(&Sha256::digest(bytes)))
    }

    /// Wrap an already-hex-encoded hash. The caller is asserting
    /// "this string is a sha256 hex digest." Used by command-specific
    /// `CasInputs` impls that delegate to legacy hashing helpers
    /// (e.g. fit's `fit_content_hash`/`fit_stage_hash`) and need to
    /// surface their output as a `ContentHash`.
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// Full 64-char hex digest.
    pub fn full(&self) -> &str { &self.0 }

    /// First 8 chars, used as the directory-name prefix.
    pub fn short(&self) -> &str { &self.0[..8.min(self.0.len())] }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

// ─── CasInputs trait ─────────────────────────────────────────────────────────

/// Every CAS-emitting subcommand's typed input set implements this for
/// a *single-realization* run (one seed, one logical instance). The
/// trait fixes:
///
/// 1. How content hashes get computed from typed inputs (no ad-hoc
///    canonical-string assembly scattered across commands).
/// 2. Where on disk a run lives (no per-command path-format
///    construction in caller code).
/// 3. What metadata gets written to `run.json` (the trait returns the
///    `RunKind` envelope ready for serialization).
///
/// Reader code only needs the trait to consume a single-realization leaf run.
pub trait CasInputs {
    /// Stable content hash. Two impls returning the same hash MUST
    /// have produced the same outputs (modulo sha256 collision
    /// resistance, which we trust).
    fn content_hash(&self) -> ContentHash;

    /// Filesystem path under the CAS root. Function of `content_hash`
    /// plus presentation hints. Two distinct content hashes MUST
    /// produce distinct paths.
    fn cas_path(&self, root: &Path) -> PathBuf;

    /// `RunKind` payload for `run.json`. Includes the kind
    /// discriminant and human-readable provenance fields.
    fn run_kind(&self) -> RunKind;

    /// Convenience: assemble a `Run` envelope from this inputs's hash
    /// + run_kind plus execution metadata (version, argv, wall time).
    /// Default impl matches what every command's write site does by
    /// hand; commands call it instead of inlining Run construction.
    /// Default impl produces a `Running` run; the caller transitions
    /// to `Completed` at end-of-run by assigning
    /// `run.status = RunStatus::Completed { wall_time_seconds: t }`.
    /// This matches the typical "write run.json early so a crashed
    /// run is still discoverable, patch wall_time at the end" pattern.
    fn to_run(&self, version: String, argv: Vec<String>) -> Run {
        Run {
            hash:              self.content_hash().full().to_string(),
            version,
            created_at:        super::iso8601_utc(std::time::SystemTime::now()),
            argv,
            status:            RunStatus::Running,
            label:             None,
            kind:              self.run_kind(),
        }
    }
}

// ─── Canonical hashing ───────────────────────────────────────────────────────

/// Compose a content hash from a sorted list of `(field, value)`
/// pairs. The hash is sha256 over `field=value\nfield=value\n…` after
/// stable sorting by field name.
///
/// This is the cheapest canonicalization that gives stable hashes
/// across argv reorderings, HashMap iteration order, and incidental
/// formatting differences. Callers pass the *content-bearing* fields
/// only — ephemeral inputs (parallel, progress) must not appear here.
pub fn hash_canonical(fields: &[(&str, &str)]) -> ContentHash {
    let mut sorted: Vec<(&str, &str)> = fields.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    let canonical: String = sorted.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");
    ContentHash::from_bytes(canonical.as_bytes())
}


// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_from_bytes_is_deterministic() {
        let a = ContentHash::from_bytes(b"hello");
        let b = ContentHash::from_bytes(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.full().len(), 64);
    }

    #[test]
    fn content_hash_short_is_eight_chars() {
        let h = ContentHash::from_bytes(b"x");
        assert_eq!(h.short().len(), 8);
        assert_eq!(h.short(), &h.full()[..8]);
    }

    #[test]
    fn hash_canonical_sorts_fields() {
        let h1 = hash_canonical(&[("b", "2"), ("a", "1")]);
        let h2 = hash_canonical(&[("a", "1"), ("b", "2")]);
        assert_eq!(h1, h2,
            "argument order must not affect canonical hash");
    }

    #[test]
    fn hash_canonical_distinguishes_field_names() {
        // Same values, different field names — must hash differently.
        let h1 = hash_canonical(&[("seed", "1")]);
        let h2 = hash_canonical(&[("dataset", "1")]);
        assert_ne!(h1, h2);
    }

}
