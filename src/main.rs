//! Smoke entry: confirms the no-false-negative invariant on a small run.
use ribbonguard::{BlockedFilter, SplitMix64};

fn main() {
    let k = 100_000usize;
    let mut f = BlockedFilter::with_buckets((k as f64 / 2.5).ceil() as usize);
    let mut rng = SplitMix64::new(42);
    let keys: Vec<u64> = (0..k).map(|_| rng.next_u64()).collect();
    for &key in &keys {
        f.insert(key);
    }
    let fn_count = keys.iter().filter(|&&x| !f.contains(x)).count();
    println!(
        "inserted={k} false_negatives={fn_count} overflow={}",
        f.overflow_len()
    );
    assert_eq!(fn_count, 0, "SACRED INVARIANT VIOLATED");
    println!("no-false-negative invariant holds.");
}
