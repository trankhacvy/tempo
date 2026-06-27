//! Shared synthetic-book generators + host-timing helpers for the Tempo benchmark.
//! The headline claim — clearing cost is `O(ticks)`, not `O(orders)` — is made
//! visible by timing `find_cross` as the tick count grows (linear) and as the order
//! count grows at fixed ticks (flat).

/// A small deterministic LCG so every benchmark input is reproducible run-to-run.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Build a crossing demand/supply histogram over `ticks` buckets from `num_orders`
/// synthetic orders. Demand skews to the upper ticks and supply to the lower ticks
/// so the book genuinely crosses; quantities are bounded so cumulative sums and the
/// `u64` matched-volume conversion never saturate.
pub fn synthetic_book(ticks: usize, num_orders: usize, seed: u64) -> (Vec<u64>, Vec<u64>) {
    let mut rng = Lcg::new(seed);
    let mut demand = vec![0u64; ticks];
    let mut supply = vec![0u64; ticks];
    if ticks == 0 {
        return (demand, supply);
    }
    let half = ticks / 2;
    for _ in 0..num_orders {
        let qty = 1 + (rng.next() % 100);
        if rng.next() & 1 == 0 {
            // demand: bias to the upper half (buyers willing to pay more)
            let t =
                half + (rng.next() as usize % ticks.max(1)).min(ticks - 1 - half.min(ticks - 1));
            demand[t.min(ticks - 1)] += qty;
        } else {
            // supply: bias to the lower half (sellers accepting less)
            let t = rng.next() as usize % (half + 1).min(ticks);
            supply[t.min(ticks - 1)] += qty;
        }
    }
    (demand, supply)
}

/// Average nanoseconds for one `find_cross` over `(demand, supply)`, across `iters`
/// calls (with a warmup). Deterministic inputs; the timing is the only nondeterminism.
pub fn time_find_cross(demand: &[u64], supply: &[u64], iters: u32) -> u128 {
    use std::hint::black_box;
    use std::time::Instant;
    let _ = tempo_math::clearing::find_cross(demand, supply);
    let start = Instant::now();
    for _ in 0..iters.max(1) {
        let _ = black_box(tempo_math::clearing::find_cross(
            black_box(demand),
            black_box(supply),
        ));
    }
    start.elapsed().as_nanos() / iters.max(1) as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_book_crosses() {
        let (d, s) = synthetic_book(256, 1024, 7);
        let r = tempo_math::clearing::find_cross(&d, &s).unwrap();
        assert!(r.crossed, "synthetic book should cross");
        assert_eq!(d.len(), 256);
        assert_eq!(s.len(), 256);
    }

    #[test]
    fn synthetic_book_is_deterministic() {
        assert_eq!(synthetic_book(64, 100, 1), synthetic_book(64, 100, 1));
    }
}
