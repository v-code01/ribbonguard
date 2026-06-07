# RibbonGuard — Finding #2 (HEADLINE): adaptive skew collapse

**Machine:** Apple M4 Pro (aarch64, NEON). Rust/cargo 1.96.0, `--release`,
`lto=fat`, `codegen-units=1`. Single-thread. All data generated locally
(splitmix64), zero downloads. This finding is **hardware-independent** — it is a
membership/statistics result, not a throughput one.

**Verdict: PROMOTE (headline), honestly bounded.** Under a skewed negative stream
the adaptive guard collapses the SUSTAINED false-positive rate by **~100x to
>10,000x at 0.01–0.1 bits/key**, with a clean, disclosed Pareto across the skew
range. Outside skew — broad/uniform/**adversarial** (all-distinct) negatives —
the guard fills, the cap evicts, and a **residual FP floor returns** (at the
extreme, the full base rate at 1.0x, pure overhead). We state exactly where it
wins and what happens outside, in the spirit of calibann finding #1
(wins high-recall, loses low-recall).

The sacred **no-false-negative** invariant holds across the *entire* sweep
(every member tests present at every configuration) — adaptation is a
pure-conjunction suppressor over confirmed non-members and can never suppress a
member (proven in the Group-1/2 harness; re-asserted at runtime here).

---

## Disclosed parameters (both honesty conditions require this)

| param | value |
|---|---|
| K (inserted members) | 1,000,000 |
| base load factor λ | 2.5 → **~1% base FP** (see `spike.md`) |
| stream length n | 10,000,000 negatives |
| sustained window | last **1,000,000** stream elements |
| guard caps swept | {512, 4096, 32768} |
| Zipf skew range s | **{0.6, 0.8, 1.0, 1.2, 1.5}** |
| negative universe U | **10,000 (hot)** and **1,000,000 (broad)** |
| ground truth | negatives drawn from a universe **provably disjoint** from members (top-bit tag), so every `contains==true` on the stream *is* a false positive and every `mark_false_positive` targets a confirmed non-member |

`bits/key = guard_bytes * 8 / K` — the DISCLOSED extra space of the adaptive
guard (a `u64` key in the set + a `u64` in the FIFO order deque per entry).
Reproduce: `cargo run --release --bin skew` (raw log: `bench_results/skew_run.txt`).

---

## HONESTY CONDITION 1 — sustained-FP-vs-bits/key Pareto across the skew RANGE

Not one cherry-picked Zipfian. Flat is the single point (0 extra bits/key, base
FP); adaptive traces a Pareto as the cap grows. Two universe sizes.

### Hot universe, U = 10,000 (the robust win)

All ~93 distinct base false positives among the 10k universe recur under skew, so
**every** cap (even 512) marks all of them; the guard holds only **93 entries**
and the sustained rate hits the measurement floor (0 FP in the last 1M).

| s | flat FP | adaptive FP | collapse | bits/key | guard |
|---:|---:|---:|---:|---:|---:|
| 0.6 | 1.1237% | 0.0000% | >11,237x (floor-limited) | 0.012 | 93 |
| 0.8 | 1.1797% | 0.0000% | >11,797x (floor-limited) | 0.012 | 93 |
| 1.0 | 1.1553% | 0.0000% | >11,553x (floor-limited) | 0.012 | 93 |
| 1.2 | 0.9799% | 0.0000% | >9,799x (floor-limited) | 0.012 | 93 |
| 1.5 | 0.5603% | 0.0000% | >5,603x (floor-limited) | 0.012 | 93 |

(caps 512/4096/32768 are identical here — the guard never exceeds 93, so the cap
never binds. `>Nx` = 0 FP measured in the window, i.e. limited by 1/1,000,000.)

**Headline point:** under skew with a bounded hot set, the sustained FP collapses
from **~1% to ~0 (>11,000x, measurement-floor-limited) at 0.012 bits/key** —
because *every* recurring FP is marked exactly once and stays suppressed.

### Broad universe, U = 1,000,000 (the Pareto is visible; skew still governs)

Here the cap binds and the Pareto is clean and finite. Note the flat rate itself
*climbs with s*: under concentrated skew a single hot element dominates the
window, and if it is a base FP the flat rate balloons (up to 52% at s=1.5). The
guard removes exactly that hot FP.

| s | config | flat FP | adaptive FP | collapse | bits/key | guard |
|---:|---|---:|---:|---:|---:|---:|
| 0.6 | cap=512 | 1.2173% | 0.8173% | 1.5x | 0.066 | 512 |
| 0.6 | cap=4096 | 1.2173% | 0.4099% | 3.0x | 0.524 | 4096 |
| 0.6 | cap=32768 | 1.2173% | 0.0027% | 450.9x | 1.229 | 9598 |
| 0.8 | cap=512 | 2.9544% | 0.6199% | 4.8x | 0.066 | 512 |
| 0.8 | cap=4096 | 2.9544% | 0.2640% | 11.2x | 0.524 | 4096 |
| 0.8 | cap=32768 | 2.9544% | 0.0114% | 259.2x | 1.188 | 9282 |
| 1.0 | cap=512 | 11.0804% | 0.2965% | 37.4x | 0.066 | 512 |
| 1.0 | cap=4096 | 11.0804% | 0.1027% | 107.9x | 0.524 | 4096 |
| 1.0 | cap=32768 | 11.0804% | 0.0254% | 436.2x | 0.942 | 7357 |
| 1.2 | cap=512 | 27.5656% | 0.0765% | 360.3x | 0.066 | 512 |
| 1.2 | cap=4096 | 27.5656% | 0.0180% | 1531.4x | 0.435 | 3402 |
| 1.2 | cap=32768 | 27.5656% | 0.0180% | 1531.4x | 0.435 | 3402 |
| 1.5 | cap=512 | 51.9663% | 0.0066% | 7873.7x | 0.066 | 512 |
| 1.5 | cap=4096 | 51.9663% | 0.0043% | 12085.2x | 0.075 | 586 |
| 1.5 | cap=32768 | 51.9663% | 0.0043% | 12085.2x | 0.075 | 586 |

**Reading the Pareto (the honest part):**
- **Low skew, broad U (s=0.6):** the cap is the bottleneck. cap=512 buys only
  **1.5x** — a **residual floor of ~0.82%** because 512 << the ~9.6k distinct FPs.
  You must spend up to **1.23 bits/key** (cap=32768) to reach 450x. This is where
  adaptation is *expensive*.
- **High skew, broad U (s≥1.2):** a handful of hot elements dominate, so even
  cap=512 gives **360–7900x** at **0.066 bits/key**, and the guard naturally stays
  small (586–3402 < cap) so bits/key stays ~0.075. Cheapest, biggest wins.
- The Pareto knee moves with **s**: more skew → more collapse per bit. There is no
  single "~100x" number; it ranges **1.5x → >12,000x** and we disclose the whole
  surface.

---

## HONESTY CONDITION 2 — regime honesty (where it wins, what happens outside)

Fixed generous cap = 32,768. Same K, λ, n, window.

| regime | flat FP | adaptive FP | collapse | bits/key | evictions |
|---|---:|---:|---:|---:|---:|
| **SKEW** s=1.5, U=10k | 0.5603% | 0.0000% | >5,603x (floor) | 0.012 | 0 |
| **SKEW** s=1.0, U=10k | 1.1553% | 0.0000% | >11,553x (floor) | 0.012 | 0 |
| BROAD zipf s=0.8, U=1M | 2.9544% | 0.0114% | 259.2x | 1.188 | 0 |
| UNIFORM, U=1M | 0.9856% | 0.0000% | >9,856x (floor) | 1.234 | 0 |
| **ADVERSARIAL, U=∞** (all-distinct) | 0.9734% | 0.9734% | **1.0x** | 4.194 | 65,368 |

**Where it WINS:** under skew (small hot U, or high s), and even under *bounded*
uniform/broad negatives **when the cap covers the distinct-FP count** — the guard
fills once and every recurring FP stays suppressed. Cost: **0.01 bits/key** in the
hot regime, **~1.2 bits/key** in the bounded-broad regime.

**What happens OUTSIDE (the disclosed cost):**
- **Bounded broad, cap < distinct FPs** (see s=0.6 cap=512 above): the cap evicts,
  suppression churns, and a **residual FP floor of ~0.82%** returns — only 1.5x
  better than flat.
- **ADVERSARIAL / unbounded U / no recurrence:** every negative is distinct, so a
  marked FP never recurs and the mark is wasted. The guard is pure churn (**65,368
  evictions**), the sustained rate stays at the **full base rate (~0.97%, 1.0x — no
  benefit)**, and adaptation costs **4.19 bits/key of dead overhead**. This is the
  hard floor: **the residual FP floor equals the flat rate**, and the space is
  spent for nothing.

So: **"~100x–>10,000x collapse UNDER SKEW at 0.01–0.1 bits/key; under broad
negatives the cost climbs to ~1.2 bits/key for a 260–450x win; under
adversarial/unbounded negatives there is NO win — a residual floor at the full
~1% base rate and 4.2 bits/key of wasted space."** No "~100x" is stated without
its skew regime and its cost.

---

## Space cost summary (bits/key)

| operating point | bits/key | note |
|---|---:|---|
| hot-U skew (guard ≈ 93) | **0.012** | robust win, cap never binds |
| high-skew broad (guard ≈ 586) | **0.075** | biggest cheap win |
| bounded-broad, full coverage (guard ≈ 7–9.6k) | **0.94–1.23** | cap must cover distinct FPs |
| adversarial (guard = cap, churning) | **4.19** | pure overhead, 0 benefit |

Base filter footprint is separate (λ=2.5 → 51.2 bits/key of fingerprints, see
`spike.md`); the numbers above are the *adaptive guard's marginal* cost.

## Correctness (sacred invariant) — re-asserted at runtime

`skew` asserts, across **every** configuration in the sweep, that all 1,000,000
members test present after the full negative stream + adaptation. Output line:
`SACRED INVARIANT across entire sweep: every member present = TRUE (no false
negatives)`. Backed by the exhaustive + stateful + adaptive-interleaved harness
(`tests/`), plus the unit test `adaptive_beats_flat_and_never_suppresses_a_member`
and `adversarial_regime_leaves_a_residual_floor` in `src/gen.rs`.
