//! RibbonGuard — NEON blocked (bucketed) approximate-membership filter.
//!
//! Design (Task-0 spike): a Vector-Quotient-Filter-style blocked AMQ.
//! - `bucket = splitmix(key) -> high bits % n_buckets`.
//! - Each bucket is 16 one-byte fingerprint slots (16 bytes), probed with ONE
//!   `vceqq_u8` 16-way compare + `vmaxvq_u8` movemax. One contiguous 16-byte load
//!   (single cache line), NO gather, NO pointer chase.
//! - Fingerprint is a nonzero byte (0 == empty slot sentinel).
//! - Overflow policy (sacred no-false-negative): if a bucket's 16 slots are all
//!   full, the key is placed in a small overflow `HashSet`. `contains` consults it
//!   only when the fast NEON probe misses AND the set is non-empty. A full bucket
//!   therefore can NEVER drop a key -> no false negatives, ever.
//!
//! Single-thread v1. NEON is baseline on aarch64, so no runtime feature detection.

use std::collections::HashSet;

/// Slots per bucket. 16 one-byte fingerprints == exactly one NEON `uint8x16`.
pub const SLOTS: usize = 16;

/// splitmix64 finalizer — cheap, well-distributed 64-bit mixing.
#[inline(always)]
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// NEON blocked filter.
pub struct BlockedFilter {
    /// Flat fingerprint store: `n_buckets * SLOTS` bytes, contiguous.
    slots: Vec<u8>,
    n_buckets: usize,
    /// Keys evicted from full buckets. Empty at target load; checked only on miss.
    overflow: HashSet<u64>,
    len: usize,
}

impl BlockedFilter {
    /// Create a filter with `n_buckets` cache-line buckets (`n_buckets*16` bytes).
    pub fn with_buckets(n_buckets: usize) -> Self {
        assert!(n_buckets > 0);
        Self {
            slots: vec![0u8; n_buckets * SLOTS],
            n_buckets,
            overflow: HashSet::new(),
            len: 0,
        }
    }

    /// bucket index and nonzero fingerprint for `key`.
    #[inline(always)]
    fn locate(&self, key: u64) -> (usize, u8) {
        let h = mix(key);
        let bucket = ((h >> 32) as usize) % self.n_buckets;
        // low byte mapped to 1..=255 (255 distinct fingerprints; 0 == empty slot).
        // Mapping 0 -> 255 keeps a full ~1/255 collision rate (does NOT waste a bit
        // the way `| 1` would, which would collapse to 128 values).
        let b = (h & 0xFF) as u8;
        let fp = if b == 0 { 0xFF } else { b };
        (bucket, fp)
    }

    /// Insert a key. Preserves the no-false-negative invariant unconditionally.
    pub fn insert(&mut self, key: u64) {
        let (bucket, fp) = self.locate(key);
        let base = bucket * SLOTS;
        let bslots = &mut self.slots[base..base + SLOTS];
        // Already present in this bucket? (idempotent, avoids dup fill)
        for &s in bslots.iter() {
            if s == fp {
                // Fingerprint collision or true dup: treat as present. Membership
                // is fingerprint-level, so no need to store twice.
                self.len += 1;
                return;
            }
        }
        for s in bslots.iter_mut() {
            if *s == 0 {
                *s = fp;
                self.len += 1;
                return;
            }
        }
        // Bucket full: overflow (sacred invariant).
        self.overflow.insert(key);
        self.len += 1;
    }

    /// NEON probe: one 16-byte load, one `vceqq_u8`, one `vmaxvq_u8`.
    #[cfg(target_arch = "aarch64")]
    #[inline(always)]
    pub fn contains(&self, key: u64) -> bool {
        use std::arch::aarch64::*;
        let (bucket, fp) = self.locate(key);
        let base = bucket * SLOTS;
        // SAFETY: base+16 <= slots.len() by construction; ptr is valid & aligned
        // enough for an unaligned vector load (vld1q_u8 has no alignment req).
        unsafe {
            let v = vld1q_u8(self.slots.as_ptr().add(base));
            let needle = vdupq_n_u8(fp);
            let eq = vceqq_u8(v, needle);
            if vmaxvq_u8(eq) != 0 {
                return true;
            }
        }
        // Fast path: overflow almost always empty at target load.
        if !self.overflow.is_empty() {
            return self.overflow.contains(&key);
        }
        false
    }

    /// Scalar fallback (non-aarch64) — same semantics.
    #[cfg(not(target_arch = "aarch64"))]
    #[inline(always)]
    pub fn contains(&self, key: u64) -> bool {
        let (bucket, fp) = self.locate(key);
        let base = bucket * SLOTS;
        for &s in &self.slots[base..base + SLOTS] {
            if s == fp {
                return true;
            }
        }
        if !self.overflow.is_empty() {
            return self.overflow.contains(&key);
        }
        false
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn overflow_len(&self) -> usize {
        self.overflow.len()
    }
    /// Total resident bytes of the fingerprint store (space accounting).
    pub fn store_bytes(&self) -> usize {
        self.slots.len()
    }
    pub fn n_buckets(&self) -> usize {
        self.n_buckets
    }
}

/// splitmix64 PRNG for self-contained key/stream generation (no deps, deterministic).
pub struct SplitMix64 {
    state: u64,
}
impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline(always)]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SACRED: no false negatives. Insert K random keys, all must be found.
    #[test]
    fn no_false_negatives() {
        let k = 200_000usize;
        // Load factor ~2.5 keys/bucket.
        let n_buckets = (k as f64 / 2.5).ceil() as usize;
        let mut f = BlockedFilter::with_buckets(n_buckets);
        let mut rng = SplitMix64::new(0xDEAD_BEEF);
        let keys: Vec<u64> = (0..k).map(|_| rng.next_u64()).collect();
        for &key in &keys {
            f.insert(key);
        }
        for &key in &keys {
            assert!(f.contains(key), "FALSE NEGATIVE for key {key:#x}");
        }
    }

    /// Adversarial: force full buckets (tiny n_buckets) — still no false negatives.
    #[test]
    fn no_false_negatives_overflow_stress() {
        let k = 50_000usize;
        let n_buckets = 64; // ~780 keys/bucket -> massive overflow
        let mut f = BlockedFilter::with_buckets(n_buckets);
        let mut rng = SplitMix64::new(0x1234);
        let keys: Vec<u64> = (0..k).map(|_| rng.next_u64()).collect();
        for &key in &keys {
            f.insert(key);
        }
        for &key in &keys {
            assert!(f.contains(key), "FALSE NEGATIVE (overflow) key {key:#x}");
        }
        assert!(f.overflow_len() > 0, "expected overflow to engage");
    }

    #[test]
    fn measures_a_plausible_fp_rate() {
        let k = 500_000usize;
        let n_buckets = (k as f64 / 2.5).ceil() as usize;
        let mut f = BlockedFilter::with_buckets(n_buckets);
        let mut rng = SplitMix64::new(1);
        for _ in 0..k {
            f.insert(rng.next_u64());
        }
        // Fresh keys disjoint from inserted (different seed stream region).
        let mut q = SplitMix64::new(0xFFFF_0000_AAAA);
        let trials = 1_000_000usize;
        let mut fp = 0usize;
        for _ in 0..trials {
            if f.contains(q.next_u64()) {
                fp += 1;
            }
        }
        let rate = fp as f64 / trials as f64;
        // ~1% expected at load 2.5; assert loosely to catch gross bugs.
        assert!(rate > 0.001 && rate < 0.05, "FP rate {rate} out of sane range");
    }
}
