# RibbonGuard — Task-0 Spike Results

**Machine:** Apple M4 Pro (aarch64, NEON), 4 P-cores + 10 E-cores, **L2 = 16 MiB** per
P-cluster. Rust/cargo 1.96.0, `--release`, `lto=fat`, `codegen-units=1`,
`RUSTFLAGS=-C target-cpu=native`. Single-thread. All data generated locally
(splitmix64), zero downloads.

**Verdict: DEMOTE** — the NEON blocked filter does **not** beat `xorf` at a *matched*
FP-rate on this M4. It wins throughput (~1.4x) only in a *relaxed-FP / small-footprint*
regime, and at large space cost. Honest three-axis Pareto below. Finding #2 (adaptive
skew) is independent and carries regardless.

_Note: numbers here were reproduced twice (two independent runs, ±2%); the tables match._

---

## Design (candidate)

Vector-Quotient-Filter-style blocked AMQ.
- `bucket = splitmix64(key)[63:32] % n_buckets`. Each bucket = **16 one-byte
  fingerprint slots** (16 bytes) = exactly one NEON `uint8x16`.
- Fingerprint = `splitmix64(key)[7:0]` mapped to `1..=255` (0 = empty-slot sentinel;
  0 is remapped to 255 so all 255 nonzero codes are used — a naive `| 1` would waste a
  bit and double the FP rate, which an early version did and was fixed).
- `contains`: **one contiguous 16-byte load** (`vld1q_u8`, single cache line), broadcast
  the needle (`vdupq_n_u8`), one 16-way `vceqq_u8`, one `vmaxvq_u8` movemax. **No gather,
  no pointer chase.**
- `insert`: place fingerprint in first empty slot. **Overflow policy (sacred):** if all
  16 slots are full the key goes to a small `HashSet`; `contains` consults it only when
  the NEON probe misses AND the set is non-empty. A full bucket can therefore **never**
  drop a key.

## Baseline

`xorf = 0.12.0`, `BinaryFuse8` (8-bit fingerprints, `TryFrom<&[u64]>`). Native FP ≈
2⁻⁸ = 0.39%; ~9.0 bits/key. This is `xorf`'s *highest-FP / lowest-space* variant — there
is no ~1% `xorf` config, so a strictly matched-FP comparison forces the NEON filter down
to 0.39%.

## Method

K = 1,000,000 distinct u64 keys. Probe stream = 16,000,000, **50% present / 50% absent,
interleaved**. Warm, best-of-7 timing, `black_box` on key + accumulated hits to defeat
DCE. FP measured on 1,000,000 fresh keys disjoint from the inserted set. bits/key =
resident fingerprint bytes × 8 / K.

## Results (representative; stable across runs, ±2%)

| filter | config | probes/s | bits/key | FP-rate | NEON/xorf |
|---|---|---:|---:|---:|---:|
| `xorf` BinaryFuse8 | native | **~610 M/s** | 9.04 | 0.39% | 1.00x |
| NEON blocked | λ=2.5 (~1% target) | **~866 M/s** | 51.2 (6.4 MiB) | 0.96% | **1.42x** |
| NEON blocked | λ=1.0 (matched FP) | **~592 M/s** | 128 (16 MiB) | 0.39% | 0.96x |

NEON load-factor sweep (λ = keys/bucket → FP, bits/key), single run:

```
λ=0.8  FP=0.32%  bits/key=160.0   (20.0 MiB)
λ=1.0  FP=0.39%  bits/key=128.0   (16.0 MiB)  <- matched to xorf
λ=1.5  FP=0.59%  bits/key= 85.3   (10.7 MiB)
λ=2.0  FP=0.77%  bits/key= 64.0   ( 8.0 MiB)
λ=2.5  FP=0.96%  bits/key= 51.2   ( 6.4 MiB)  <- ~1% operating point
λ=3.0  FP=1.17%  bits/key= 42.7   ( 5.3 MiB)
λ=8.0  FP=3.09%  bits/key= 16.0   ( 2.0 MiB, overflow engages)
```

## Why (the honest systems reason)

The probe cost is a single random bucket load + one NEON compare — cheap *if the bucket
is L2-resident*. The design's space cost is fundamental: 8 bits per slot, slots kept
sparse to hold FP down (FP ≈ λ/255), so **bits/key = 128/λ**.

- At ~1% FP (λ=2.5) the whole filter is **6.4 MiB**, well inside the M4's 16 MiB L2 →
  the single-load NEON probe beats `xorf`'s multiple semi-random fingerprint loads: **1.42x**.
- To *match* `xorf`'s 0.39% FP the NEON filter needs λ≈1.0 = **16 MiB = exactly L2 size**.
  It falls off the cache cliff; miss latency dominates and the SIMD advantage is
  cancelled: **0.96x** (a hair *below* `xorf`, which stays L2-resident at 9 MiB).

So the NEON filter trades space for throughput and cannot win both. At **equal FP** it
loses; at **equal footprint** it would win throughput but with worse FP. A real, measured
Pareto surface, not a wash — no cherry-pick.

## Honest regime where NEON wins

**FP target ≳ ~0.6% (footprint ≤ ~10 MiB, fits M4 P-cluster L2): NEON blocked probe is
1.3–1.5x `xorf` throughput, at 5–9x the space.** Outside that (matched sub-0.4% FP) it
ties-to-slightly-loses and costs ~14x the space.

## Correctness (sacred invariant)

**NO FALSE NEGATIVES — confirmed.** `cargo test --release`, all green:
- `no_false_negatives`: 200k random keys inserted, all found.
- `no_false_negatives_overflow_stress`: 50k keys into 64 buckets (forced massive
  overflow), all found; overflow path exercised.
- `measures_a_plausible_fp_rate`: FP within sane bounds.
- `main` smoke: 100k keys, 0 false negatives.

## Verdict

**DEMOTE.** NEON blocked filter vs `xorf` BinaryFuse8 on M4: **~866 M/s vs ~610 M/s
(1.42x) at ~1% FP** but **5.7x the space**; at **matched 0.39% FP it is ~592 M/s vs
~610 M/s (0.96x)** and **14x the space**. It wins throughput only when a relaxed FP target
keeps its footprint in L2. Reported honestly. Finding #2 (adaptive skew, ~100x sustained
FP collapse under Zipfian) is independent and unaffected.
