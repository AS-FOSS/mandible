//! Deterministic, dependency-free randomness for the audit instrument
//! (`xtask::audit`, `xtask::queue`): a seeded shuffle used both by the
//! frozen queue's shuffle-stratification (`queue::shuffle_stratify`, batch:
//! frozen sampling queue) and, historically, by the live stratified draw it
//! replaced. Extracted to its own module because both `audit.rs` and
//! `queue.rs` need the exact same seeded-shuffle behavior and must never
//! silently drift into two slightly different PRNGs.
//!
//! The workspace carries no `rand` dependency, and none of this needs
//! cryptographic quality — only that the same seed always produces the same
//! output and different seeds produce (with overwhelming probability)
//! different output. SplitMix64 is the standard, well-analyzed choice for
//! exactly that: one multiply-xor-shift step per call, no external state
//! beyond a single u64.

pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `0..n`. Not perfectly unbiased (the classic
    /// modulo-reduction skew), which is irrelevant here: `n` is a tool
    /// count in the thousands at most, `u64::MAX / n` is astronomically
    /// larger, and the property this whole module needs is
    /// reproducibility, not cryptographic uniformity.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }
}

/// Deterministic Fisher-Yates shuffle, seeded. Same `seed` and `items`, in
/// the same starting order, always produces the same permutation.
pub(crate) fn seeded_shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64::new(seed);
    for i in (1..items.len()).rev() {
        let j = rng.below(i + 1);
        items.swap(i, j);
    }
}

/// Derive a per-stratum seed from a run's own seed and the stratum's own
/// name, via a small FNV-1a mix. Without this, shuffling every stratum with
/// the *same* raw seed would make the strata's internal orders correlated
/// (the same relative shuffle pattern applied to each), which is a subtler
/// but real form of non-independence in the draw.
pub(crate) fn stratum_seed(seed: u64, stratum: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in stratum.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0001_0000_01B3);
    }
    h
}

/// Plain FNV-1a over raw bytes, no seed mixed in beforehand — used where a
/// stable, order-sensitive fingerprint is wanted (a queue's population
/// hash, `queue::population_hash`; a tie-break key,
/// `queue::shuffle_stratify`) rather than a shuffle key. Deliberately not
/// `std::collections::hash_map::DefaultHasher`: its algorithm is explicitly
/// *not* guaranteed stable across Rust releases, which would silently
/// change every previously-recorded population hash and defeat the one
/// property that value exists to give — the same tool list always hashes
/// the same, on any machine, on any compiler version, forever.
pub(crate) fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0001_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_shuffle_is_deterministic() {
        let mut a = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut b = [1, 2, 3, 4, 5, 6, 7, 8];
        seeded_shuffle(&mut a, 42);
        seeded_shuffle(&mut b, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn seeded_shuffle_actually_permutes() {
        let mut a: Vec<u32> = (0..50).collect();
        let original = a.clone();
        seeded_shuffle(&mut a, 7);
        assert_ne!(
            a, original,
            "a 50-element shuffle landing on the identity is astronomically unlikely"
        );
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted, original,
            "shuffling must not lose or duplicate elements"
        );
    }

    #[test]
    fn stratum_seed_differs_per_stratum_and_per_seed() {
        assert_ne!(stratum_seed(1, "ok"), stratum_seed(1, "low-confidence"));
        assert_ne!(stratum_seed(1, "ok"), stratum_seed(2, "ok"));
    }

    #[test]
    fn fnv1a64_is_deterministic_and_order_sensitive() {
        assert_eq!(fnv1a64(b"abc"), fnv1a64(b"abc"));
        assert_ne!(fnv1a64(b"abc"), fnv1a64(b"acb"));
        assert_ne!(fnv1a64(b"ab"), fnv1a64(b"abc"));
    }
}
