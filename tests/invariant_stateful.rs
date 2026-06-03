//! GROUP 1 — the invariant HARNESS, enforcement #2: STATEFUL property test.
//!
//! THE SACRED INVARIANT: an inserted key must ALWAYS test present. False
//! positives allowed; false negatives NEVER. Where the exhaustive harness
//! (`invariant_exhaustive.rs`) brute-forces tiny universes, this file drives
//! LONG random interleavings of `insert` against a `HashSet` shadow oracle on
//! large filters, and asserts the shadow ⊆ filter membership relation holds.
//!
//! Two enforcers:
//!   `stateful_insert_shadow` — proptest: random bucket counts (small enough to
//!      force overflow) + long random key sequences from a bounded domain (so
//!      collisions AND duplicates AND overflow all occur); after building,
//!      EVERY shadow member must be `contains()==true`. Each insert is also
//!      checked to be immediately present (monotonicity of the invariant).
//!   `long_run_large_filter` — one deterministic multi-million-op interleaving
//!      on a large filter, the "at scale" stress the proptest cases can't reach.

use proptest::prelude::*;
use ribbonguard::{BlockedFilter, SplitMix64};
use std::collections::HashSet;

proptest! {
    // High case count: the invariant is load-bearing, so we spend cases on it.
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    /// Long random insert interleaving vs a shadow set. `n_buckets` is kept small
    /// relative to the key volume so buckets saturate and the overflow path (the
    /// no-false-negative source of truth) is exercised. Keys are drawn from a
    /// bounded domain so fingerprint collisions and exact duplicates recur.
    #[test]
    fn stateful_insert_shadow(
        n_buckets in 1usize..=64,
        keys in prop::collection::vec(0u64..20_000u64, 0..6000),
    ) {
        let mut f = BlockedFilter::with_buckets(n_buckets);
        let mut shadow: HashSet<u64> = HashSet::new();
        for &k in &keys {
            f.insert(k);
            shadow.insert(k);
            // Monotonicity: the key just inserted must be present RIGHT NOW.
            prop_assert!(f.contains(k), "FALSE NEGATIVE immediately after insert: {k:#x}");
        }
        // Final sweep: every shadow member must still be present.
        for &k in &shadow {
            prop_assert!(f.contains(k), "FALSE NEGATIVE at final sweep: {k:#x}");
        }
    }
}

/// A single very long interleaving on a LARGE filter — the scale the bounded
/// proptest cases cannot reach. 2M inserts drawn from a wide random stream into
/// a filter loaded past its 16 slots/bucket, then a full shadow sweep.
/// Deterministic (fixed seed) so any regression reproduces exactly. Load is set
/// to ~20 keys/bucket so buckets saturate their 16 slots and the overflow path
/// (the no-false-negative source of truth) is stressed at scale — a distinct
/// regime from the tiny-filter overflow cases in the exhaustive harness.
#[test]
fn long_run_large_filter() {
    let n_ops = 2_000_000usize;
    // ~20 distinct keys/bucket > 16 slots → guaranteed, heavy overflow at scale.
    let n_buckets = n_ops / 20;
    let mut f = BlockedFilter::with_buckets(n_buckets);
    let mut shadow: HashSet<u64> = HashSet::with_capacity(n_ops);
    let mut rng = SplitMix64::new(0x5EED_1234_ABCD);

    for _ in 0..n_ops {
        let k = rng.next_u64();
        f.insert(k);
        shadow.insert(k);
    }
    // The sacred sweep: no inserted key may ever be reported absent.
    for &k in &shadow {
        assert!(f.contains(k), "FALSE NEGATIVE at scale for key {k:#x}");
    }
    // Sanity: the filter really did engage overflow at this load (harness must
    // exercise the risky path, not just the happy path).
    assert!(f.overflow_len() > 0, "expected overflow to engage at load ~4");
}
