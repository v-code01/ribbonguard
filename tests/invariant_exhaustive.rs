//! GROUP 1 — the invariant HARNESS, enforcement #1: EXHAUSTIVE small-universe.
//!
//! THE SACRED INVARIANT: an inserted key must ALWAYS test present. False
//! positives are allowed; false negatives are NEVER allowed. This file proves
//! it by brute force over a tiny key universe and tiny filters, deliberately
//! chosen to force BOTH fingerprint collisions (few buckets) AND full-bucket
//! overflow (more distinct keys per bucket than the 16 slots hold).
//!
//! Strategy (defense in depth):
//!   L1 `exhaustive_all_subsets_collision` — ALL 2^16 subsets of a 16-key
//!      universe across n_buckets ∈ {1,2,3}; heavy collisions, exhaustive.
//!   L2 `exhaustive_all_subsets_overflow`  — ALL 2^18 subsets of an 18-key
//!      universe with n_buckets=1; large subsets exceed the 16 slots and force
//!      overflow. Asserts overflow genuinely engaged over the enumeration.
//!   L3 `explicit_full_universe_overflow`  — the WHOLE 0..256 universe inserted
//!      into tiny filters (n_buckets ∈ {1,2,3,4}); massive, guaranteed overflow.
//!   L4 `sampled_subsets_full_universe`    — many deterministically-generated
//!      random subsets of the full 0..256 universe into tiny filters.
//! Every layer asserts: EVERY inserted key `contains()==true` after the build.

use ribbonguard::{BlockedFilter, SplitMix64};

/// Build a filter from an explicit key list and assert the sacred invariant:
/// every inserted key must test present. Returns whether overflow engaged, so
/// callers can assert the overflow path was actually exercised.
fn assert_all_present(keys: &[u64], n_buckets: usize) -> bool {
    let mut f = BlockedFilter::with_buckets(n_buckets);
    for &k in keys {
        f.insert(k);
    }
    for &k in keys {
        assert!(
            f.contains(k),
            "FALSE NEGATIVE: key {k:#x} inserted but absent (n_buckets={n_buckets}, set={keys:?})"
        );
    }
    f.overflow_len() > 0
}

/// L1 — genuinely exhaustive over ALL 2^16 subsets of a 16-key universe, with
/// only a handful of buckets so fingerprint collisions are dense. This is the
/// tightest correctness net: every possible small filling is checked.
#[test]
fn exhaustive_all_subsets_collision() {
    const U: u32 = 16; // universe = keys 0..16
    for &n_buckets in &[1usize, 2, 3] {
        for mask in 0u32..(1u32 << U) {
            let keys: Vec<u64> = (0..U).filter(|i| mask & (1 << i) != 0).map(|i| i as u64).collect();
            assert_all_present(&keys, n_buckets);
        }
    }
}

/// L2 — genuinely exhaustive over ALL 2^19 subsets of a 19-key universe routed
/// into a SINGLE bucket (n_buckets=1). A single bucket has exactly 16 slots, so
/// any subset whose members carry >16 distinct fingerprints spills into the
/// overflow set — the precise place the no-false-negative invariant is at risk.
/// The 19-key ordered universe carries 17 distinct fingerprints (measured), so
/// the full subset overflows and the overflow path is provably exercised.
#[test]
fn exhaustive_all_subsets_overflow() {
    const U: u32 = 19; // universe = keys 0..19, all into bucket 0
    let mut overflow_seen = false;
    for mask in 0u32..(1u32 << U) {
        let keys: Vec<u64> = (0..U).filter(|i| mask & (1 << i) != 0).map(|i| i as u64).collect();
        overflow_seen |= assert_all_present(&keys, 1);
    }
    assert!(
        overflow_seen,
        "expected full-bucket overflow to engage somewhere in the 2^18 enumeration"
    );
}

/// L3 — the ENTIRE 0..256 universe inserted into tiny filters. With so few
/// buckets every bucket is saturated far past its 16 slots, so overflow is
/// guaranteed and heavy. Explicitly covers the full-bucket-overflow config.
#[test]
fn explicit_full_universe_overflow() {
    let all: Vec<u64> = (0u64..256).collect();
    for &n_buckets in &[1usize, 2, 3, 4] {
        let overflowed = assert_all_present(&all, n_buckets);
        assert!(
            overflowed,
            "the full 0..256 universe in {n_buckets} bucket(s) must overflow"
        );
    }
}

/// L4 — many deterministically-generated random subsets of the full 0..256
/// universe into tiny filters. Complements the exhaustive layers by sampling the
/// combinatorially-huge full-universe subset space broadly, still under tiny
/// filters that keep overflow in play.
#[test]
fn sampled_subsets_full_universe() {
    let mut rng = SplitMix64::new(0x0A11_CE5E_ED00);
    let mut overflow_seen = false;
    for _ in 0..200_000 {
        // Each key in 0..256 is included with ~50% probability via a random bit.
        let r0 = rng.next_u64();
        let r1 = rng.next_u64();
        let r2 = rng.next_u64();
        let r3 = rng.next_u64();
        let words = [r0, r1, r2, r3];
        let keys: Vec<u64> = (0u64..256)
            .filter(|&k| (words[(k >> 6) as usize] >> (k & 63)) & 1 == 1)
            .collect();
        let n_buckets = 1 + (rng.next_u64() % 4) as usize; // tiny: 1..=4
        overflow_seen |= assert_all_present(&keys, n_buckets);
    }
    assert!(overflow_seen, "sampled full-universe subsets should hit overflow");
}
