//! Self-contained, deterministic data generators for the skew finding (#2).
//!
//! Everything here is generated from a `splitmix64` stream — zero downloads,
//! zero datasets. The load-bearing property is EXACT GROUND TRUTH: the member
//! set and the true-negative universe are made provably DISJOINT by a single
//! reserved tag bit, so every element of any negative stream is a CONFIRMED
//! non-member. That is exactly the precondition `AdaptiveFilter::mark_false_positive`
//! relies on (see `adaptive` module docs), so the harness upholds it structurally.
//!
//! Domain partition (the disjointness guarantee):
//! - MEMBER   values always have the top bit CLEAR: domain `[0, 2^63)`.
//! - NEGATIVE values always have the top bit SET:   domain `[2^63, 2^64)`.
//!
//! A member can therefore never equal a negative, independent of any hash
//! collisions downstream. This is asserted in the tests.

use crate::{AdaptiveFilter, SplitMix64};
use std::collections::HashSet;

/// Top bit set — the NEGATIVE tag. Members clear it; negatives set it.
const NEG_TAG: u64 = 1u64 << 63;
/// Mask that clears the top bit — the MEMBER domain mask.
const MEMBER_MASK: u64 = !NEG_TAG;

/// Draw `count` DISTINCT `u64` values from a `splitmix64` stream, forcing each
/// into `[0, 2^63)` (top bit clear) so it lives in the MEMBER domain.
///
/// Distinctness is enforced with a `HashSet`; at a 2^63 domain the rejection
/// rate is negligible.
pub fn member_set(count: usize, seed: u64) -> Vec<u64> {
    let mut rng = SplitMix64::new(seed);
    let mut seen: HashSet<u64> = HashSet::with_capacity(count * 2);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let v = rng.next_u64() & MEMBER_MASK;
        if seen.insert(v) {
            out.push(v);
        }
    }
    out
}

/// Draw `size` DISTINCT `u64` values in the NEGATIVE domain `[2^63, 2^64)`
/// (top bit set). By construction every returned value is disjoint from any
/// [`member_set`] output, so it is a CONFIRMED non-member.
pub fn negative_universe(size: usize, seed: u64) -> Vec<u64> {
    let mut rng = SplitMix64::new(seed);
    let mut seen: HashSet<u64> = HashSet::with_capacity(size * 2);
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let v = rng.next_u64() | NEG_TAG;
        if seen.insert(v) {
            out.push(v);
        }
    }
    out
}

/// A Zipfian rank sampler over `U` ranks with exponent `s`: `P(rank i) ∝ i^-s`
/// for `i in 1..=U`. Precomputes the normalized CDF once; each draw is one
/// `f64` uniform + a binary search (`O(log U)`).
///
/// `s == 0` degenerates to uniform. Larger `s` == heavier head (more skew).
pub struct Zipf {
    /// Normalized inclusive prefix sums; `cdf[U-1] == 1.0`, strictly increasing.
    cdf: Vec<f64>,
}

impl Zipf {
    /// Build the CDF for `u` ranks at exponent `s`. `u` must be non-zero.
    pub fn new(u: usize, s: f64) -> Self {
        assert!(u > 0, "Zipf universe must be non-empty");
        let mut cdf = Vec::with_capacity(u);
        let mut acc = 0.0f64;
        for i in 1..=u {
            acc += (i as f64).powf(-s);
            cdf.push(acc);
        }
        let z = acc;
        for c in cdf.iter_mut() {
            *c /= z;
        }
        Self { cdf }
    }

    /// Number of ranks in this sampler.
    #[inline]
    pub fn len(&self) -> usize {
        self.cdf.len()
    }

    /// Always non-empty by construction (`new` asserts `u > 0`).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    /// Map a uniform draw `x in [0, 1)` to a rank index in `[0, U)`.
    /// Returns the first index whose inclusive CDF is `>= x` (rank 0 == hottest).
    #[inline]
    pub fn rank(&self, x: f64) -> usize {
        // partition_point returns the count of leading elements with cdf < x,
        // i.e. the first index with cdf >= x. Clamp for the x -> 1.0 boundary.
        let idx = self.cdf.partition_point(|&c| c < x);
        idx.min(self.cdf.len() - 1)
    }
}

/// Uniform `f64` in `[0, 1)` from 53 random bits of a `splitmix64` word.
#[inline]
fn unit(rng: &mut SplitMix64) -> f64 {
    // Top 53 bits -> exact multiples of 2^-53 in [0, 1).
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// A Zipfian negative stream of length `n`: each element is
/// `universe[zipf_rank]`, hottest ranks recurring most. Skew `s`, universe
/// size `U = universe.len()`. Every element is a confirmed non-member (the
/// universe is disjoint from members by construction).
pub fn zipf_stream(universe: &[u64], s: f64, n: usize, seed: u64) -> Vec<u64> {
    let zipf = Zipf::new(universe.len(), s);
    let mut rng = SplitMix64::new(seed);
    (0..n).map(|_| universe[zipf.rank(unit(&mut rng))]).collect()
}

/// A uniform negative stream: each element an equiprobable draw from `universe`.
/// (Broad-negative regime: `U` large, no head to exploit.)
pub fn uniform_stream(universe: &[u64], n: usize, seed: u64) -> Vec<u64> {
    let u = universe.len();
    assert!(u > 0, "uniform stream needs a non-empty universe");
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| universe[(rng.next_u64() as usize) % u])
        .collect()
}

/// An adversarial negative stream: `n` ALL-DISTINCT confirmed non-members
/// (effectively an infinite universe). No element ever recurs, so the guard
/// can never amortize a mark — the worst case for adaptation.
pub fn adversarial_stream(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = SplitMix64::new(seed);
    let mut seen: HashSet<u64> = HashSet::with_capacity(n * 2);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let v = rng.next_u64() | NEG_TAG;
        if seen.insert(v) {
            out.push(v);
        }
    }
    out
}

/// Outcome of replaying a negative stream through one filter configuration.
pub struct StreamOutcome {
    /// Sustained false-positive rate over the FINAL `window` stream elements.
    pub sustained_fp: f64,
    /// Guard occupancy at end of stream (suppressed confirmed FPs held).
    pub guard_len: usize,
    /// Extra resident bytes of the guard (drives the bits/key disclosure).
    pub guard_bytes: usize,
    /// FIFO evictions forced by the cap over the run (drives the residual floor).
    pub evictions: u64,
    /// SACRED invariant re-check: every member still tests present at the end.
    pub members_present: bool,
}

/// Build an [`AdaptiveFilter`] over `members` at load factor `lambda`, replay
/// the negative `stream`, and measure the SUSTAINED false-positive rate over
/// the last `window` elements.
///
/// - `adapt == false`: FLAT baseline — the guard is never engaged.
/// - `adapt == true`:  on each false positive, `mark_false_positive` is called
///   with the offending element. That element is a CONFIRMED non-member (the
///   stream is drawn from a universe disjoint from `members`), so the
///   precondition is upheld by construction — EXACT ground truth.
///
/// Every stream element is a non-member, so any `contains == true` IS a false
/// positive; no oracle lookup is needed. Members are swept at the end to
/// re-assert the sacred no-false-negative invariant under adaptation.
pub fn replay(
    members: &[u64],
    lambda: f64,
    cap: usize,
    adapt: bool,
    stream: &[u64],
    window: usize,
) -> StreamOutcome {
    let n_buckets = ((members.len() as f64 / lambda).ceil() as usize).max(1);
    let effective_cap = if adapt { cap } else { 0 };
    let mut f = AdaptiveFilter::with_buckets(n_buckets, effective_cap);
    for &m in members {
        f.insert(m);
    }

    let n = stream.len();
    let window = window.min(n).max(1);
    let start = n - window;
    let mut fp_window = 0u64;
    for (i, &neg) in stream.iter().enumerate() {
        if f.contains(neg) {
            if i >= start {
                fp_window += 1;
            }
            if adapt {
                // Exact ground truth: `neg` is a confirmed non-member.
                f.mark_false_positive(neg);
            }
        }
    }

    let members_present = members.iter().all(|&m| f.contains(m));
    StreamOutcome {
        sustained_fp: fp_window as f64 / window as f64,
        guard_len: f.guard_len(),
        guard_bytes: f.guard_bytes(),
        evictions: f.evictions(),
        members_present,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EXACT GROUND TRUTH: members and every negative generator are provably
    /// disjoint (member top bit 0, negative top bit 1) — and empirically so.
    #[test]
    fn negatives_are_disjoint_from_members() {
        let members = member_set(50_000, 0xA11CE);
        let universe = negative_universe(50_000, 0xBEEF);
        let adv = adversarial_stream(20_000, 0xF00D);

        // Tag-bit guarantee (this is what makes disjointness provable).
        assert!(members.iter().all(|&m| m & NEG_TAG == 0), "member tag bit");
        assert!(universe.iter().all(|&v| v & NEG_TAG != 0), "negative tag bit");
        assert!(adv.iter().all(|&v| v & NEG_TAG != 0), "adversarial tag bit");

        // Empirical set-intersection is empty.
        let mset: HashSet<u64> = members.iter().copied().collect();
        assert!(
            universe.iter().all(|v| !mset.contains(v)),
            "negative universe overlaps members"
        );
        assert!(
            adv.iter().all(|v| !mset.contains(v)),
            "adversarial stream overlaps members"
        );

        // Generators honor their distinctness contract.
        assert_eq!(members.iter().copied().collect::<HashSet<_>>().len(), 50_000);
        assert_eq!(universe.iter().copied().collect::<HashSet<_>>().len(), 50_000);
        assert_eq!(adv.iter().copied().collect::<HashSet<_>>().len(), 20_000);
    }

    /// The Zipfian frequencies match the exponent: the empirical count ratio of
    /// rank 0 to rank r approximates the theoretical `(r+1)^s` within tolerance.
    #[test]
    fn zipf_frequencies_match_exponent() {
        let u = 128usize;
        let n = 4_000_000usize;
        for &s in &[0.8f64, 1.0, 1.5] {
            let universe = negative_universe(u, 0xC0FFEE ^ (s.to_bits()));
            let stream = zipf_stream(&universe, s, n, 0x5EED);

            // Count occurrences of the three hottest universe values.
            let (mut c0, mut c1, mut c2) = (0u64, 0u64, 0u64);
            for &x in &stream {
                if x == universe[0] {
                    c0 += 1;
                } else if x == universe[1] {
                    c1 += 1;
                } else if x == universe[2] {
                    c2 += 1;
                }
            }
            assert!(c1 > 1000 && c2 > 1000, "insufficient samples on tail ranks");

            // rank0/rank1 ~ 2^s, rank0/rank2 ~ 3^s.
            let r01 = c0 as f64 / c1 as f64;
            let r02 = c0 as f64 / c2 as f64;
            let t01 = 2.0f64.powf(s);
            let t02 = 3.0f64.powf(s);
            assert!(
                (r01 / t01 - 1.0).abs() < 0.15,
                "s={s}: rank0/rank1={r01:.3} vs 2^s={t01:.3}"
            );
            assert!(
                (r02 / t02 - 1.0).abs() < 0.15,
                "s={s}: rank0/rank2={r02:.3} vs 3^s={t02:.3}"
            );
        }
    }

    /// SACRED + WIN, at unit scale: on a small skewed stream the ADAPTIVE
    /// sustained-FP is strictly below FLAT, AND no member is ever suppressed.
    #[test]
    fn adaptive_beats_flat_and_never_suppresses_a_member() {
        let k = 20_000usize;
        let u = 2_000usize; // small hot universe -> recurrence to exploit
        let n = 2_000_000usize;
        let window = 200_000usize;
        let lambda = 2.5; // ~1% base FP operating point

        let members = member_set(k, 0x1111);
        let universe = negative_universe(u, 0x2222);
        let stream = zipf_stream(&universe, 1.2, n, 0x3333);

        let flat = replay(&members, lambda, 0, false, &stream, window);
        let adaptive = replay(&members, lambda, 4096, true, &stream, window);

        // Sacred: adaptation never suppresses a member (both configs).
        assert!(flat.members_present, "flat suppressed a member (impossible)");
        assert!(
            adaptive.members_present,
            "adaptive suppressed a member — SACRED INVARIANT VIOLATED"
        );

        // Win under skew: strictly below flat.
        assert!(
            adaptive.sustained_fp < flat.sustained_fp,
            "adaptive {} not below flat {}",
            adaptive.sustained_fp,
            flat.sustained_fp
        );
        // The flat baseline must actually exhibit a non-trivial FP rate, else
        // the comparison is vacuous.
        assert!(flat.sustained_fp > 0.0, "flat FP rate was zero — vacuous");
    }

    /// Broad/adversarial regime honesty: under an all-distinct stream the guard
    /// churns (evictions > 0) and a residual FP floor remains — adaptation does
    /// NOT collapse to ~0 here. Members still never suppressed.
    #[test]
    fn adversarial_regime_leaves_a_residual_floor() {
        let k = 20_000usize;
        let n = 1_000_000usize;
        let window = 200_000usize;
        let lambda = 2.5;

        let members = member_set(k, 0x4444);
        let stream = adversarial_stream(n, 0x5555);

        let flat = replay(&members, lambda, 0, false, &stream, window);
        let adaptive = replay(&members, lambda, 512, true, &stream, window);

        assert!(adaptive.members_present, "member suppressed under adversarial");
        // Cap forces eviction under all-distinct negatives.
        assert!(adaptive.evictions > 0, "expected cap eviction churn");
        // A residual floor remains: adaptation cannot drive FP to ~0 with no
        // recurrence. It stays a meaningful fraction of the flat rate.
        assert!(
            adaptive.sustained_fp > flat.sustained_fp * 0.25,
            "adversarial adaptive {} unexpectedly far below flat {} (should floor)",
            adaptive.sustained_fp,
            flat.sustained_fp
        );
    }
}
