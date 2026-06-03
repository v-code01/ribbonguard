# Task-0 spike — NEON blocked filter vs xorf BinaryFuse8 (Apple M4)

**Verdict: regime win (honest), not a clean win.** The NEON single-cache-line probe is faster than xorf at a loose FP target, but only by spending far more space; at matched FP, xorf wins on both axes. Finding #1 is therefore a caveated throughput-for-space result. The project's headline is finding #2 (skew-adaptive FP collapse), which is hardware-independent and does not depend on this ratio.

Self-contained: 1,000,000 splitmix64 keys, 16,000,000-probe stream (50% present / 50% disjoint true-negative), best of 7, single-thread, release. No false negatives on either filter (verified in-bench and by the lib invariant tests).

| operating point | probes/s | bits/key | FP-rate |
|---|---|---|---|
| xorf BinaryFuse8 | 617 M/s | 9.04 | 0.40% |
| NEON blocked @ ~1% FP (lambda=2.5) | **870 M/s (1.41x)** | 51.2 (5.6x) | 0.96% |
| NEON blocked @ matched FP (lambda=1) | 586 M/s (0.95x) | 128.0 (14x) | 0.39% |

NEON blocked filter load-factor sweep (FP vs space):

| lambda (keys/bucket) | FP-rate | bits/key | overflow |
|---|---|---|---|
| 1.0 | 0.39% | 128.0 | 0 |
| 2.0 | 0.77% | 64.0 | 0 |
| 2.5 | 0.96% | 51.2 | 0 |
| 3.0 | 1.17% | 42.7 | 0 |
| 4.0 | 1.55% | 32.0 | 0 |
| 6.0 | 2.33% | 21.3 | 16 |
| 8.0 | 3.09% | 16.0 | 598 |

## Honest analysis
- The NEON probe is one contiguous 16-byte load + `vceqq_u8` + `vmaxvq_u8` (one cache line, no gather). xorf BinaryFuse8 does 3 independent fingerprint lookups (3 cache lines). That is why NEON is 1.41x faster at ~1% FP.
- But a blocked filter with 16 one-byte slots per bucket at low load wastes most slots, so it costs 51 bits/key at ~1% FP vs xorf's 9. To match xorf's 0.40% FP it needs lambda=1 (128 bits/key), where the speed edge is gone (0.95x). At matched FP, xorf dominates on speed AND space.
- So RibbonGuard's blocked filter is a THROUGHPUT-FOR-SPACE filter: it wins probe rate where FP can be loose and RAM is cheap; it is not space-competitive with a fuse filter. Stated plainly, no cherry-pick.
- Sacred invariant: no false negatives on either filter (overflow policy engages only at lambda>=6 here and never drops a key).

## Consequence for the build
Finding #1 ships as this honest speed/space Pareto (not "beats xorf"). Finding #2 (skew-adaptive) is the headline and is hardware-independent. The load-bearing gate remains the no-false-negative invariant, re-checked under adaptation.
