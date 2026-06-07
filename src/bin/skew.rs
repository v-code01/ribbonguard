//! Finding #2 (HEADLINE): the adaptive layer collapses SUSTAINED false-positive
//! rate under a SKEWED negative stream vs the flat filter — reported honestly
//! against the two Proxy conditions.
//!
//! HONESTY CONDITION 1 — disclosed params + a Pareto across a skew RANGE:
//!   sweep the Zipf exponent s over {0.6, 0.8, 1.0, 1.2, 1.5} and the negative
//!   universe size U over {small hot, broad}, and report the
//!   sustained-FP-vs-bits/key Pareto for flat vs adaptive (several guard caps)
//!   at each s. Not one cherry-picked Zipfian. s, U, cap, K all stated.
//!
//! HONESTY CONDITION 2 — regime honesty:
//!   show the win UNDER SKEW (small hot U, high s) AND the behavior under
//!   BROAD/UNIFORM/ADVERSARIAL negatives (large/infinite U): the guard fills,
//!   the cap evicts, and a RESIDUAL FP FLOOR returns.
//!
//! EXACT GROUND TRUTH: negatives are drawn from a universe provably disjoint
//! from the member set (top-bit tag), so every `contains == true` on the stream
//! IS a false positive and every `mark_false_positive` targets a confirmed
//! non-member. Self-contained (splitmix64), single-thread, `--release` only.

use std::hint::black_box;
use std::time::Instant;

use ribbonguard::generators::{
    adversarial_stream, member_set, negative_universe, replay, uniform_stream, zipf_stream,
    StreamOutcome,
};

/// Extra guard bits per inserted key (the disclosed space cost of adaptation).
fn bits_per_key(guard_bytes: usize, k: usize) -> f64 {
    guard_bytes as f64 * 8.0 / k as f64
}

/// Collapse factor flat/adaptive, guarding the measurement floor (0 FP in the
/// window => report the ratio against a 1/window floor and flag it).
fn collapse(flat: f64, adaptive: f64, window: usize) -> String {
    if adaptive <= 0.0 {
        let floored = flat / (1.0 / window as f64);
        format!(">{floored:.0}x (adaptive < 1/{window}, measurement-floor-limited)")
    } else {
        format!("{:.1}x", flat / adaptive)
    }
}

fn pct(x: f64) -> String {
    format!("{:.4}%", x * 100.0)
}

fn main() {
    // ---- disclosed parameters ----
    let k: usize = 1_000_000; // inserted members
    let lambda: f64 = 2.5; // base load factor => ~1% base FP (see spike.md)
    let n: usize = 10_000_000; // negative stream length
    let window: usize = 1_000_000; // trailing window for the SUSTAINED rate
    let caps: [usize; 3] = [512, 4_096, 32_768]; // guard capacities (Pareto knob)
    let s_range: [f64; 5] = [0.6, 0.8, 1.0, 1.2, 1.5]; // disclosed skew range
    let u_small: usize = 10_000; // small HOT universe (skew regime)
    let u_broad: usize = 1_000_000; // broad universe (non-skew regime)

    const SEED_M: u64 = 0x0003_1BB6_0000_0001;
    const SEED_U: u64 = 0x0003_1BB6_0000_0002;
    const SEED_STREAM: u64 = 0x0003_1BB6_0000_0003;
    const SEED_UNIF: u64 = 0x0003_1BB6_0000_0004;
    const SEED_ADV: u64 = 0x0003_1BB6_0000_0005;

    eprintln!("[gen] {k} members, stream n={n}, window={window}");
    let members = member_set(k, SEED_M);
    let uni_small = negative_universe(u_small, SEED_U);
    let uni_broad = negative_universe(u_broad, SEED_U ^ 0xF);

    println!("=== RibbonGuard Finding #2 — adaptive skew collapse (Apple M4, aarch64) ===");
    println!("DISCLOSED PARAMS:");
    println!("  K (members)          : {k}");
    println!("  base load factor λ   : {lambda}  (=> ~1% base FP, per spike.md)");
    println!("  stream length n      : {n}");
    println!("  sustained window     : last {window} elements");
    println!("  guard caps swept     : {caps:?}");
    println!("  skew range s         : {s_range:?}");
    println!("  U (hot / broad)      : {u_small} / {u_broad}");
    println!("  ground truth         : negatives disjoint from members (top-bit tag)");
    println!();

    // A running assertion: no member is ever suppressed anywhere in the sweep.
    let mut all_members_present = true;

    // ================= HONESTY CONDITION 1 — Pareto across the skew range =====
    for (label, universe, u) in [("HOT U", &uni_small, u_small), ("BROAD U", &uni_broad, u_broad)] {
        println!("---- CONDITION 1: sustained-FP vs bits/key Pareto — {label} = {u} ----");
        println!(
            "  {:>4} {:>8} | {:>10} | {:>10} {:>10} | {:>9} {:>8}",
            "s", "config", "flat FP", "adapt FP", "collapse", "bits/key", "guard"
        );
        for &s in &s_range {
            let stream = zipf_stream(universe, s, n, SEED_STREAM);
            let flat = replay(&members, lambda, 0, false, &stream, window);
            all_members_present &= flat.members_present;
            println!(
                "  {s:>4.1} {:>8} | {:>10} | {:>10} {:>10} | {:>9} {:>8}",
                "flat",
                pct(flat.sustained_fp),
                "-",
                "1.0x",
                "0.000",
                "0"
            );
            for &cap in &caps {
                let a: StreamOutcome = replay(&members, lambda, cap, true, &stream, window);
                all_members_present &= a.members_present;
                println!(
                    "  {s:>4.1} {:>8} | {:>10} | {:>10} {:>10} | {:>9.3} {:>8}",
                    format!("cap={cap}"),
                    pct(flat.sustained_fp),
                    pct(a.sustained_fp),
                    collapse(flat.sustained_fp, a.sustained_fp, window),
                    bits_per_key(a.guard_bytes, k),
                    a.guard_len,
                );
            }
        }
        println!();
    }

    // ================= HONESTY CONDITION 2 — regime honesty ===================
    // Fixed generous cap; contrast the skew win vs broad/uniform/adversarial.
    let cap = 32_768usize;
    println!("---- CONDITION 2: regime honesty (cap={cap}) — where it wins, what happens outside ----");
    println!(
        "  {:>26} | {:>10} {:>10} {:>10} | {:>9} {:>10}",
        "regime", "flat FP", "adapt FP", "collapse", "bits/key", "evictions"
    );

    let mut regime = |name: &str, stream: &[u64]| {
        let flat = replay(&members, lambda, 0, false, stream, window);
        let a = replay(&members, lambda, cap, true, stream, window);
        all_members_present &= flat.members_present && a.members_present;
        println!(
            "  {:>26} | {:>10} {:>10} {:>10} | {:>9.3} {:>10}",
            name,
            pct(flat.sustained_fp),
            pct(a.sustained_fp),
            collapse(flat.sustained_fp, a.sustained_fp, window),
            bits_per_key(a.guard_bytes, k),
            a.evictions,
        );
    };

    // WIN: skew — small hot U, high s.
    let skew_stream = zipf_stream(&uni_small, 1.5, n, SEED_STREAM);
    regime("SKEW  s=1.5 U=10k", &skew_stream);
    let skew_stream2 = zipf_stream(&uni_small, 1.0, n, SEED_STREAM);
    regime("SKEW  s=1.0 U=10k", &skew_stream2);

    // OUTSIDE: broad Zipf, uniform, adversarial (all-distinct).
    let broad_zipf = zipf_stream(&uni_broad, 0.8, n, SEED_STREAM);
    regime("BROAD zipf s=0.8 U=1M", &broad_zipf);
    let unif = uniform_stream(&uni_broad, n, SEED_UNIF);
    regime("UNIFORM      U=1M", &unif);
    let adv = adversarial_stream(n, SEED_ADV);
    regime("ADVERSARIAL  U=inf", &adv);

    println!();
    println!(
        "SACRED INVARIANT across entire sweep: every member present = {}",
        if all_members_present { "TRUE (no false negatives)" } else { "FALSE — VIOLATION" }
    );
    assert!(all_members_present, "SACRED no-false-negative invariant violated in sweep");
    black_box(all_members_present);

    // Timestamp so re-runs are self-evidently fresh.
    eprintln!("[done] {:?}", Instant::now());
}
