//! Task-0 spike bench: NEON blocked filter vs `xorf` BinaryFuse8 on Apple M4.
//! Self-contained: all keys/streams generated locally. `--release` only.

use std::hint::black_box;
use std::time::Instant;
use xorf::{BinaryFuse8, Filter};

use ribbonguard::{BlockedFilter, SplitMix64};

/// Measure FP rate of a NEON filter at a given bucket count over fresh keys.
fn measure_fp_neon(f: &BlockedFilter, absent: &[u64]) -> f64 {
    let mut fp = 0usize;
    for &k in absent {
        if f.contains(k) {
            fp += 1;
        }
    }
    fp as f64 / absent.len() as f64
}

/// Build a NEON filter at target load factor lambda (keys/bucket).
fn build_neon(keys: &[u64], lambda: f64) -> BlockedFilter {
    let n_buckets = (keys.len() as f64 / lambda).ceil() as usize;
    let mut f = BlockedFilter::with_buckets(n_buckets.max(1));
    for &k in keys {
        f.insert(k);
    }
    f
}

/// Time `contains` over the full stream; return probes/sec (best of `reps`).
fn time_probes<F: Fn(u64) -> bool>(stream: &[u64], reps: usize, probe: F) -> (f64, u64) {
    let mut best = 0.0f64;
    let mut sink = 0u64;
    for _ in 0..reps {
        let t = Instant::now();
        let mut hits = 0u64;
        for &k in stream {
            // black_box the key and accumulate to defeat DCE.
            hits += probe(black_box(k)) as u64;
        }
        let dt = t.elapsed().as_secs_f64();
        sink = sink.wrapping_add(hits);
        let pps = stream.len() as f64 / dt;
        if pps > best {
            best = pps;
        }
    }
    (best, sink)
}

fn main() {
    // ---- parameters ----
    let k: usize = 1_000_000; // inserted keys
    let n_probes: usize = 16_000_000; // mixed query stream length
    let reps: usize = 7;

    eprintln!("[gen] {k} keys, {n_probes} probe stream");

    // ---- self-contained key generation (distinct u64) ----
    let mut rng = SplitMix64::new(0x21BB_0000_0000_0001u64);
    let mut keys: Vec<u64> = Vec::with_capacity(k);
    {
        use std::collections::HashSet;
        let mut seen = HashSet::with_capacity(k * 2);
        while keys.len() < k {
            let x = rng.next_u64();
            if seen.insert(x) {
                keys.push(x);
            }
        }
    }

    // ---- absent (fresh) keys for FP measurement, disjoint from inserted ----
    let mut arng = SplitMix64::new(0xAB5E_0000_C0DE_0001);
    let mut absent: Vec<u64> = Vec::with_capacity(1_000_000);
    {
        use std::collections::HashSet;
        let inserted: HashSet<u64> = keys.iter().copied().collect();
        while absent.len() < 1_000_000 {
            let x = arng.next_u64();
            if !inserted.contains(&x) {
                absent.push(x);
            }
        }
    }

    // ---- xorf baseline ----
    let t = Instant::now();
    let xf = BinaryFuse8::try_from(&keys).expect("xorf build");
    let xf_build = t.elapsed().as_secs_f64();
    let xf_bits_per_key = xf.len() as f64 * 8.0 / k as f64;
    let xf_fp = {
        let mut fp = 0usize;
        for &a in &absent {
            if xf.contains(&a) {
                fp += 1;
            }
        }
        fp as f64 / absent.len() as f64
    };
    eprintln!(
        "[xorf] BinaryFuse8 build={xf_build:.3}s fingerprints={} bits/key={xf_bits_per_key:.3} FP={:.4}%",
        xf.len(),
        xf_fp * 100.0
    );

    // ---- NEON load-factor sweep to find matched-FP operating points ----
    eprintln!("[neon] load-factor sweep (lambda -> FP, bits/key):");
    let mut sweep = Vec::new();
    for &lambda in &[0.8f64, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 6.0, 8.0] {
        let f = build_neon(&keys, lambda);
        let fp = measure_fp_neon(&f, &absent);
        let bpk = f.store_bytes() as f64 * 8.0 / k as f64;
        eprintln!(
            "        lambda={lambda:>4} FP={:.4}% bits/key={bpk:6.2} overflow={}",
            fp * 100.0,
            f.overflow_len()
        );
        sweep.push((lambda, fp, bpk));
    }

    // Pick config closest to ~1% target and config closest to xorf's FP.
    let pick = |target: f64| -> f64 {
        sweep
            .iter()
            .min_by(|a, b| {
                (a.1 - target)
                    .abs()
                    .partial_cmp(&(b.1 - target).abs())
                    .unwrap()
            })
            .unwrap()
            .0
    };
    let lambda_1pct = pick(0.01);
    let lambda_matched = pick(xf_fp);

    // ---- build mixed probe stream (present + absent interleaved, deterministic) ----
    // 50% present (sampled from inserted), 50% absent (fresh). Interleaved so the
    // branch predictor sees a realistic mix, not a sorted run.
    let mut srng = SplitMix64::new(0x57EA_0000_0001);
    let mut stream: Vec<u64> = Vec::with_capacity(n_probes);
    for i in 0..n_probes {
        if i & 1 == 0 {
            let idx = (srng.next_u64() as usize) % keys.len();
            stream.push(keys[idx]);
        } else {
            let idx = (srng.next_u64() as usize) % absent.len();
            stream.push(absent[idx]);
        }
    }

    // ---- throughput: xorf ----
    let (xf_pps, s1) = time_probes(&stream, reps, |k| xf.contains(&k));

    // ---- throughput: NEON at 1% and at matched-FP ----
    let f1 = build_neon(&keys, lambda_1pct);
    let f1_fp = measure_fp_neon(&f1, &absent);
    let f1_bpk = f1.store_bytes() as f64 * 8.0 / k as f64;
    let (neon1_pps, s2) = time_probes(&stream, reps, |k| f1.contains(k));

    let fm = build_neon(&keys, lambda_matched);
    let fm_fp = measure_fp_neon(&fm, &absent);
    let fm_bpk = fm.store_bytes() as f64 * 8.0 / k as f64;
    let (neonm_pps, s3) = time_probes(&stream, reps, |k| fm.contains(k));

    black_box((s1, s2, s3));

    // ---- report ----
    println!("=== RibbonGuard Task-0 Spike Results (Apple M4 Pro, aarch64/NEON) ===");
    println!("K inserted keys      : {k}");
    println!("probe stream length  : {n_probes} (50% present / 50% absent), best of {reps}");
    println!();
    println!("xorf BinaryFuse8:");
    println!("  probes/s   : {:.3} M/s", xf_pps / 1e6);
    println!("  bits/key   : {xf_bits_per_key:.3}");
    println!("  FP-rate    : {:.4}%", xf_fp * 100.0);
    println!();
    println!("NEON blocked filter @ ~1% target (lambda={lambda_1pct}):");
    println!("  probes/s   : {:.3} M/s", neon1_pps / 1e6);
    println!("  bits/key   : {f1_bpk:.3}");
    println!("  FP-rate    : {:.4}%", f1_fp * 100.0);
    println!("  overflow   : {}", f1.overflow_len());
    println!();
    println!("NEON blocked filter @ matched-FP (lambda={lambda_matched}, target xorf FP):");
    println!("  probes/s   : {:.3} M/s", neonm_pps / 1e6);
    println!("  bits/key   : {fm_bpk:.3}");
    println!("  FP-rate    : {:.4}%", fm_fp * 100.0);
    println!("  overflow   : {}", fm.overflow_len());
    println!();
    let ratio_1 = neon1_pps / xf_pps;
    let ratio_m = neonm_pps / xf_pps;
    println!("throughput ratio NEON/xorf @1%       : {ratio_1:.2}x");
    println!("throughput ratio NEON/xorf @matchedFP: {ratio_m:.2}x");
}
