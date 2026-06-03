//! Skew-adaptive AMQ layer that suppresses RECURRING false positives —
//! PROVABLY without introducing false negatives.
//!
//! # The design (provably-safe by construction)
//!
//! `AdaptiveFilter` wraps the base [`BlockedFilter`] with a bounded `guard`: a
//! set of CONFIRMED FALSE POSITIVES (i.e. confirmed NON-members that the base
//! filter nonetheless reports present).
//!
//! - `insert(key)`      → `base.insert(key)`; the guard is never touched here.
//! - `contains(key)`    → `base.contains(key) && !guard_contains(key)`.
//! - `mark_false_positive(key)` → add `key` to the guard.
//!   PRECONDITION (the workload oracle's job): `key` is a CONFIRMED NON-member.
//!
//! Under skew the same hot negatives recur, so each hot false positive is
//! suppressed permanently after being marked exactly once — the sustained
//! false-positive rate collapses.
//!
//! # THE MECHANISM ARGUMENT — why no false negative is possible, ever
//!
//! The guard is, by its documented precondition, a set that only ever holds
//! CONFIRMED NON-members. Take any inserted member `m`. Because `m` is a member
//! and the guard contains only non-members, `m` can never be an element of the
//! guard, so `guard_contains(m) == false` and therefore `!guard_contains(m) ==
//! true`. Meanwhile the base filter already satisfies the sacred invariant:
//! `base.contains(m) == true` for every inserted `m` (proven exhaustively in
//! the Group-1 harness). Hence:
//!
//! ```text
//! contains(m) = base.contains(m) && !guard_contains(m) = true && true = true.
//! ```
//!
//! The `&& !guard_contains` term is a pure conjunction with a value that is
//! *provably true for every member*, so adaptation can only ever flip a
//! reported PRESENT to ABSENT for a key in the guard — and no member is ever in
//! the guard. Adaptation therefore CANNOT suppress a member. No false negatives,
//! by construction, independent of the guard's size, contents ordering, or
//! eviction policy. The only thing eviction affects is *which* false positives
//! stay suppressed (an FP-rate/space tradeoff), never correctness.
//!
//! # Bounded capacity + eviction (for regime honesty)
//!
//! The guard is capped at `cap` entries with FIFO eviction. Under a small hot
//! negative set (skew) the guard holds every hot FP and suppression is total.
//! Under broad/uniform negatives the guard fills and churns (FIFO), so evicted
//! FPs can recur → a residual FP floor. That floor is the honest regime cost
//! disclosed in Group 3; it is a rate concern only and never a correctness one.

use crate::BlockedFilter;
use std::collections::{HashSet, VecDeque};

/// Skew-adaptive filter: a base [`BlockedFilter`] plus a bounded guard of
/// confirmed false positives. See the module docs for the no-false-negative
/// mechanism argument.
pub struct AdaptiveFilter {
    /// The underlying no-false-negative AMQ. All membership starts here.
    base: BlockedFilter,
    /// Confirmed NON-members to suppress. INVARIANT (caller-guaranteed via the
    /// `mark_false_positive` precondition): every element is a non-member, so no
    /// inserted member is ever present here.
    guard: HashSet<u64>,
    /// FIFO insertion order of `guard` members, for bounded-capacity eviction.
    /// Kept in lockstep with `guard`: same elements, no duplicates.
    order: VecDeque<u64>,
    /// Maximum guard size. `cap == 0` disables adaptation entirely.
    cap: usize,
    /// Total accepted `mark_false_positive` events (observability).
    marks: u64,
    /// Total evictions forced by the cap (observability; drives the FP floor).
    evictions: u64,
}

impl AdaptiveFilter {
    /// Build an adaptive filter over a base of `n_buckets` cache-line buckets,
    /// with a guard capped at `cap` confirmed false positives. `cap == 0`
    /// disables suppression (behaves exactly like the base filter).
    pub fn with_buckets(n_buckets: usize, cap: usize) -> Self {
        Self {
            base: BlockedFilter::with_buckets(n_buckets),
            guard: HashSet::new(),
            order: VecDeque::new(),
            cap,
            marks: 0,
            evictions: 0,
        }
    }

    /// Insert a member. Delegates straight to the base; the guard is untouched,
    /// so insertion can never interact with suppression.
    #[inline]
    pub fn insert(&mut self, key: u64) {
        self.base.insert(key);
    }

    /// Membership query. `base.contains(key) && !guard_contains(key)`.
    ///
    /// For any inserted member the second term is provably `true` (the guard
    /// holds only confirmed non-members), so members always test present — no
    /// false negatives. See the module-level mechanism argument.
    #[inline]
    pub fn contains(&self, key: u64) -> bool {
        // Short-circuit: if the base says absent, we're done (and definitely no
        // false negative to worry about). Only base-positives consult the guard.
        self.base.contains(key) && !self.guard.contains(&key)
    }

    /// Suppress a confirmed false positive.
    ///
    /// # Precondition (workload oracle)
    /// `key` MUST be a confirmed NON-member (disjoint from every inserted key).
    /// This is the caller's contract; upholding it is what makes the guard a
    /// set of non-members, which is what makes false negatives impossible. This
    /// method cannot and does not verify membership — the base filter is
    /// approximate — so the guarantee rests on this precondition being honored
    /// by the caller's exact-truth oracle (as the harness does).
    pub fn mark_false_positive(&mut self, key: u64) {
        // Adaptation disabled, or already suppressed: nothing to do. (Re-marking
        // an already-guarded key is idempotent and does NOT refresh its FIFO
        // position — keeps `order`/`guard` in perfect lockstep with no dups.)
        if self.cap == 0 || self.guard.contains(&key) {
            return;
        }
        // Enforce the capacity bound with FIFO eviction before inserting. `while`
        // (not `if`) is defensive: if `cap` were ever lowered mid-life it drains
        // down correctly.
        while self.guard.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.guard.remove(&old);
                self.evictions += 1;
            } else {
                break; // guard empty but len>=cap only if cap==0, already handled
            }
        }
        self.guard.insert(key);
        self.order.push_back(key);
        self.marks += 1;
    }

    /// Immutable view of the underlying base filter (space/stats passthrough).
    #[inline]
    pub fn base(&self) -> &BlockedFilter {
        &self.base
    }
    /// Current number of suppressed false positives held in the guard.
    #[inline]
    pub fn guard_len(&self) -> usize {
        self.guard.len()
    }
    /// Configured guard capacity.
    #[inline]
    pub fn cap(&self) -> usize {
        self.cap
    }
    /// Total accepted mark events over this filter's life.
    #[inline]
    pub fn marks(&self) -> u64 {
        self.marks
    }
    /// Total FIFO evictions forced by the cap (drives the residual FP floor).
    #[inline]
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
    /// Extra resident bytes of the guard beyond the base store (rough space
    /// accounting for the bits/key disclosure): one `u64` key each in the set
    /// and the order deque.
    #[inline]
    pub fn guard_bytes(&self) -> usize {
        self.guard.len() * std::mem::size_of::<u64>()
            + self.order.len() * std::mem::size_of::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplitMix64;

    /// Members always present, even after marking many confirmed non-members —
    /// the core no-false-negative property at the unit level.
    #[test]
    fn members_survive_arbitrary_marks() {
        let mut f = AdaptiveFilter::with_buckets(32, 16);
        let members: Vec<u64> = (0..500).collect();
        for &m in &members {
            f.insert(m);
        }
        // Mark a pile of confirmed non-members (disjoint high domain).
        for nm in 1_000_000u64..1_002_000 {
            f.mark_false_positive(nm);
            // A cheap running check on a couple of members after each event.
            assert!(f.contains(0));
            assert!(f.contains(499));
        }
        // Full sweep: nothing marked could have suppressed a member.
        for &m in &members {
            assert!(f.contains(m), "false negative for member {m}");
        }
        // Cap respected.
        assert!(f.guard_len() <= f.cap());
        assert!(f.evictions() > 0, "cap of 16 vs 2000 marks must evict");
    }

    /// Marking an actual base false positive suppresses it — adaptation works,
    /// and does so without disturbing members that share its cache-line block.
    #[test]
    fn suppresses_a_real_false_positive() {
        let mut f = AdaptiveFilter::with_buckets(64, 4096);
        let members: Vec<u64> = (0..2000).collect();
        for &m in &members {
            f.insert(m);
        }
        // Find a confirmed non-member the base reports present (a real FP).
        let mut rng = SplitMix64::new(0x9);
        let mut found = None;
        for _ in 0..1_000_000 {
            let cand = 10_000_000 + (rng.next_u64() % 10_000_000);
            if !members.contains(&cand) && f.contains(cand) {
                found = Some(cand);
                break;
            }
        }
        let fp = found.expect("should find at least one false positive");
        assert!(f.contains(fp), "precondition: fp reads present before marking");
        f.mark_false_positive(fp);
        assert!(!f.contains(fp), "marked FP must now be suppressed");
        // And every member is still present.
        for &m in &members {
            assert!(f.contains(m), "false negative for member {m} after suppression");
        }
    }

    /// `cap == 0` disables adaptation: marks are no-ops, base semantics intact.
    #[test]
    fn zero_cap_disables_adaptation() {
        let mut f = AdaptiveFilter::with_buckets(8, 0);
        f.insert(42);
        f.mark_false_positive(1_000);
        assert_eq!(f.guard_len(), 0);
        assert_eq!(f.marks(), 0);
        assert!(f.contains(42));
    }
}
