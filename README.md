# ribbonguard

A NEON blocked approximate-membership filter fused with a skew-adaptive false-positive suppressor, in Rust, measured on Apple M4. The base filter is a Vector-Quotient-Filter-style blocked AMQ that probes one cache line with a single NEON compare. The guard sits on top and learns which false positives recur, then suppresses exactly those. The sacred invariant across both layers: no false negatives, ever.

All numbers below are measured on Apple M4 Pro (aarch64, NEON, single-thread, `--release`, `lto=fat`, `codegen-units=1`, `target-cpu=native`), on data generated locally with splitmix64, zero downloads. Every number maps to committed evidence in `bench_results/` via `claims.toml`. Where a headline could be overstated, the honest bound is stated in the same line.

## Finding 1: the NEON probe is a throughput-for-space filter, not a strict win over xorf

The base filter buckets a key into 16 one-byte fingerprint slots (exactly one `uint8x16`), then `contains` does one contiguous 16-byte load (`vld1q_u8`), one broadcast (`vdupq_n_u8`), one 16-way compare (`vceqq_u8`), one movemax (`vmaxvq_u8`). One cache line, no gather, no pointer chase. Baseline is `xorf` `BinaryFuse8` (8-bit fingerprints, ~9.0 bits/key, native FP 0.39%), its lowest-space variant. Full method and load-factor sweep in `bench_results/spike.md`.

| filter | config | probes/s | bits/key | FP-rate | NEON/xorf |
|---|---|---:|---:|---:|---:|
| xorf BinaryFuse8 | native | ~610 M/s | 9.04 | 0.39% | 1.00x |
| NEON blocked | λ=2.5 (~1% target) | ~866 M/s | 51.2 (6.4 MiB) | 0.96% | 1.42x |
| NEON blocked | λ=1.0 (matched FP) | ~592 M/s | 128 (16 MiB) | 0.39% | 0.96x |

Read this as a Pareto, not as "beats xorf". At a relaxed ~1% FP target the NEON probe is 1.42x xorf throughput, but it pays 5.7x the space (51.2 vs 9.04 bits/key). To match xorf's 0.39% FP the filter needs λ=1.0, which is 16 MiB, exactly the M4 P-cluster L2 size; it falls off the cache cliff and lands at 0.96x, a wash, at 14x the space. The win is real and L2-residency-bounded: the single-load probe beats xorf's multiple semi-random fingerprint loads only while the whole filter (~6.4 MiB at λ=2.5) stays inside L2. It buys throughput with space and cannot win both axes at once.

## Finding 2 (headline): under skew the adaptive guard collapses sustained false positives, and it is honest about where it does not

The guard holds a bounded set of confirmed false positives (confirmed non-members that the base filter reports present). `contains` returns true only if the base probe hits AND the key is not in the guard. Because the guard holds only confirmed non-members, it can never suppress a member. A workload oracle calls `mark_false_positive` when it confirms a base hit was wrong. Full sweep, parameters, and ground-truth construction in `bench_results/skew.md`.

Disclosed parameters: K = 1,000,000 members, base load factor λ = 2.5 (~1% base FP), stream n = 10,000,000 negatives, sustained window = last 1,000,000 elements, guard caps swept {512, 4096, 32768}, Zipf skew s {0.6, 0.8, 1.0, 1.2, 1.5}, negative universe U ∈ {10,000 hot, 1,000,000 broad}. Negatives are drawn from a universe provably disjoint from members (top-bit tag), so every base hit on the stream is a genuine false positive.

Hot universe (U = 10,000): the ~93 distinct base false positives all recur, so every cap marks all of them and the guard holds only 93 entries. Sustained FP collapses from ~1% to the measurement floor.

| s | flat FP | adaptive FP | collapse | bits/key |
|---:|---:|---:|---:|---:|
| 0.6 | 1.1237% | 0.0000% | >11,237x (floor) | 0.012 |
| 1.0 | 1.1553% | 0.0000% | >11,553x (floor) | 0.012 |
| 1.5 | 0.5603% | 0.0000% | >5,603x (floor) | 0.012 |

Headline: under skew with a bounded hot set, sustained FP goes from ~1% to ~0, more than 11,000x, measurement-floor-limited (0 FP in the last 1M, floor = 1/1,000,000), at 0.012 bits/key of guard.

Broad universe (U = 1,000,000): the cap binds and the Pareto is finite and clean. Across the sweep the collapse spans 1.5x to more than 12,000x by skew and cap. At low skew (s=0.6, cap=512) the guard covers only 512 of ~9,600 distinct false positives, so it buys just 1.5x and leaves a ~0.82% residual floor; reaching 450x costs up to 1.23 bits/key. At high skew (s≥1.2) a handful of hot elements dominate, so even cap=512 gives 360x to 7,900x at 0.066 bits/key. More skew means more collapse per bit; there is no single "~100x" number.

Adversarial regime (all-distinct negatives, unbounded U, generous cap 32,768): every negative is distinct, so a marked false positive never recurs and the mark is wasted. The guard is pure churn (65,368 evictions), sustained FP stays at the full base rate (0.9734%, 1.0x, no benefit), and adaptation costs 4.194 bits/key of dead overhead. This is the hard floor: with no recurrence the residual FP floor equals the flat rate and the space is spent for nothing.

So the honest statement is: ~11,000x sustained collapse under hot skew at 0.012 bits/key; a 1.5x to >12,000x Pareto under broad negatives with cost climbing to ~1.23 bits/key for the deep wins; and no win at all under adversarial all-distinct negatives, where it costs 4.2 bits/key for nothing. No "~100x" without its regime and its cost.

## The sacred invariant: no false negatives

Every inserted member tests present, at every layer, always. The base filter guarantees it with an overflow policy: if all 16 slots in a bucket are full, the key spills to a `HashSet` that `contains` consults on a probe miss, so a full bucket can never drop a key. The guard preserves it structurally: it is a pure conjunction (`base_hit AND NOT suppressed`) over confirmed non-members, so it can never suppress a member. How it is proven:

- Exhaustive small-universe check (`tests/invariant_exhaustive.rs`): every key over a small universe inserted and confirmed present.
- Stateful property test (`tests/invariant_stateful.rs`): randomized insert/probe sequences, proptest-driven, member presence held as an invariant.
- Adaptive-interleaved check (`tests/invariant_adaptive.rs`): the invariant re-checked under live adaptation, so suppression is verified never to touch a member.
- The `skew` binary re-asserts across every configuration in the sweep that all 1,000,000 members test present after the full negative stream plus adaptation.

## Status and limitations

- Single-thread throughout. No concurrency claims are made.
- Finding 1's throughput win is L2-residency-bounded and measured on this M4 (16 MiB P-cluster L2); the crossover point moves with cache size and scale.
- Finding 2's guarantee needs a workload oracle that confirms false positives so the guard can mark them. Without recurrence (adversarial all-distinct negatives) there is no win, only overhead.
- All numbers are measured on Apple M4 Pro. Finding 2 is a membership/statistics result and hardware-independent; Finding 1 is a throughput result and is not.

## License

MIT. See `LICENSE`.
