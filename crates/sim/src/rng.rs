//! A tiny deterministic PRNG (SplitMix64) so trader behaviour is reproducible from
//! a seed and the strategy stays unit-testable. No floats, matching the program's
//! no-float discipline; off-chain only.

#[derive(Clone, Debug)]
pub struct SimRng {
    state: u64,
}

impl SimRng {
    pub fn new(seed: u64) -> Self {
        // Avoid the degenerate all-zero state.
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Next raw 64-bit value (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi]` inclusive. Returns `lo` when `hi <= lo`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo + 1;
        lo + self.next_u64() % span
    }

    pub fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// A coin weighted by `pct` in `[0, 100]`.
    pub fn chance(&mut self, pct: u64) -> bool {
        self.range(0, 99) < pct.min(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproducible_for_a_fixed_seed() {
        let mut a = SimRng::new(42);
        let mut b = SimRng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SimRng::new(1);
        let mut b = SimRng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn range_stays_in_bounds() {
        let mut r = SimRng::new(7);
        for _ in 0..10_000 {
            let v = r.range(5, 9);
            assert!((5..=9).contains(&v));
        }
        assert_eq!(r.range(3, 3), 3);
        assert_eq!(r.range(9, 2), 9); // hi <= lo collapses to lo
    }
}
