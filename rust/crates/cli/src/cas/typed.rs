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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

    /// Full 64-char hex digest.
    pub fn full(&self) -> &str { &self.0 }
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

}
