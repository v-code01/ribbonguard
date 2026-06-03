//! GROUP 2 — the invariant HARNESS RE-RUN UNDER ADAPTATION (load-bearing).
//!
//! The Group-1 harness proved the base filter never yields a false negative.
//! This file proves the ADAPTIVE layer preserves that under adaptation events:
//! no `mark_false_positive` sequence (over confirmed non-members) can ever
//! suppress an inserted member. This is exactly where the sacred invariant is
//! most at risk, so it is enforced the same two ways as Group 1 — exhaustively
//! and with a stateful property test — with `mark_false_positive` interleaved.
//!
//! MECHANISM (see `src/adaptive.rs` for the full argument): the guard holds
//! ONLY confirmed non-members, so `!guard_contains(member)` is always true and
//! `contains(member) = base.contains(member) && true = true`, always. These
//! tests are the empirical cross-check of that proof.
//!
//!   `adaptive_exhaustive_subsets_marks` — enumerate insertion sets AND, for
//!      each, mark EVERY confirmed non-member one at a time, asserting every
//!      member is present after EVERY adaptive event.
//!   `adaptive_overflow_real_fp_marks`   — heavy-overflow base; mark REAL base
//!      false positives (non-members sharing a member's bucket/fingerprint), the
//!      adversarial cache-line-collision case; assert members survive + FP gets
//!      suppressed.
//!   `adaptive_stateful_shadow`          — proptest interleaving of `insert` and
//!      `mark_false_positive` (marks drawn from a disjoint never-inserted domain,
//!      so they are always confirmed non-members) vs a HashSet shadow; every
//!      shadow member present after every op.

use proptest::prelude::*;
use ribbonguard::{AdaptiveFilter, BlockedFilter, SplitMix64};
use std::collections::HashSet;

/// Exhaustive: for every insertion subset of a tiny universe, mark every
/// confirmed non-member of that universe one at a time, and after EACH mark
/// assert every inserted member still tests present. Small bucket counts make
/// marked non-members collide in bucket/fingerprint with members — the precise
/// risk the mechanism argument rules out. A small `cap` also forces eviction so
/// the guard-churn path is exercised under the invariant check.
#[test]
fn adaptive_exhaustive_subsets_marks() {
    const U: u32 = 14; // universe = keys 0..14
    const CAP: usize = 6; // < |non-members| in many subsets → forces eviction
    for &n_buckets in &[1usize, 2, 3] {
        for mask in 0u32..(1u32 << U) {
            let members: Vec<u64> =
                (0..U).filter(|i| mask & (1 << i) != 0).map(|i| i as u64).collect();
            let non_members: Vec<u64> =
                (0..U).filter(|i| mask & (1 << i) == 0).map(|i| i as u64).collect();

            let mut f = AdaptiveFilter::with_buckets(n_buckets, CAP);
            for &m in &members {
                f.insert(m);
            }
            // Adaptive events: mark each confirmed non-member; re-check ALL
            // members after every single event.
            for &nm in &non_members {
                f.mark_false_positive(nm);
                for &m in &members {
                    assert!(
                        f.contains(m),
                        "FALSE NEGATIVE under adaptation: member {m} suppressed after \
                         marking non-member {nm} (n_buckets={n_buckets}, members={members:?})"
                    );
                }
            }
        }
    }
}

/// Adversarial overflow case: a heavily-overflowed base (whole 0..256 universe
/// into ONE bucket), then mark the base's REAL false positives — confirmed
/// non-members that share the members' single cache-line block and read present.
/// This is the exact "hot negative co-resident with members" scenario. Assert
/// every member survives each suppression, and that each marked FP is suppressed.
#[test]
fn adaptive_overflow_real_fp_marks() {
    let members: Vec<u64> = (0u64..256).collect();
    let member_set: HashSet<u64> = members.iter().copied().collect();

    // Reference base (no guard) to identify genuine false positives.
    let mut base = BlockedFilter::with_buckets(1);
    for &m in &members {
        base.insert(m);
    }
    assert!(base.overflow_len() > 0, "expected heavy overflow at n_buckets=1");

    // Collect confirmed non-members that the base reports PRESENT (real FPs).
    let mut rng = SplitMix64::new(0xF00D_BEEF_1357);
    let mut real_fps: Vec<u64> = Vec::new();
    while real_fps.len() < 400 {
        let cand = 100_000 + (rng.next_u64() % 10_000_000);
        if !member_set.contains(&cand) && base.contains(cand) {
            real_fps.push(cand);
        }
    }

    // Small cap → the guard evicts while we keep marking; correctness must hold
    // regardless of eviction.
    let mut f = AdaptiveFilter::with_buckets(1, 64);
    for &m in &members {
        f.insert(m);
    }
    for &fp in &real_fps {
        assert!(f.contains(fp), "precondition: real FP reads present pre-mark");
        f.mark_false_positive(fp);
        assert!(!f.contains(fp), "marked real FP must be suppressed");
        // The sacred re-check after EVERY adaptive event.
        for &m in &members {
            assert!(
                f.contains(m),
                "FALSE NEGATIVE: member {m} suppressed after marking co-resident FP {fp}"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1200, ..ProptestConfig::default() })]

    /// Stateful: long random interleaving of `insert` and `mark_false_positive`
    /// vs a HashSet shadow. Members are drawn from a LOW domain; marks from a
    /// HIGH domain that is NEVER inserted, so every mark target is a guaranteed
    /// confirmed non-member (satisfies the `mark_false_positive` precondition
    /// structurally — "marks only from keys NOT in the shadow"). Small bucket
    /// counts + small cap keep overflow and guard-eviction both in play. After
    /// every op, every shadow member must be present.
    #[test]
    fn adaptive_stateful_shadow(
        n_buckets in 1usize..=64,
        cap in 0usize..=128,
        // op = (is_mark, value_index); value drawn from the matching domain.
        // Bounded length: the "re-check the WHOLE shadow after EVERY op" sweep
        // below is O(ops^2), so keep ops modest but the case count high.
        ops in prop::collection::vec((any::<bool>(), 0u64..10_000u64), 0..400),
    ) {
        const MARK_BASE: u64 = 1_000_000; // disjoint from the [0,10_000) members
        let mut f = AdaptiveFilter::with_buckets(n_buckets, cap);
        let mut shadow: HashSet<u64> = HashSet::new();

        for (is_mark, v) in ops {
            if is_mark {
                // Confirmed non-member by construction (never inserted).
                let nm = MARK_BASE + v;
                prop_assert!(!shadow.contains(&nm)); // precondition sanity
                f.mark_false_positive(nm);
            } else {
                f.insert(v);
                shadow.insert(v);
                prop_assert!(f.contains(v), "FALSE NEGATIVE right after insert: {v}");
            }
            // After EVERY op (insert or adaptive mark) the whole shadow holds.
            for &m in &shadow {
                prop_assert!(
                    f.contains(m),
                    "FALSE NEGATIVE under adaptation for shadow member {m}"
                );
            }
        }
    }
}
