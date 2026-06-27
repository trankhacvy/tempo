//! Mark-price derivation + per-slot brake. Mirror of `program/src/mark.rs`.

use crate::error::MathError;

/// Compute the mark price from the round's clearing prices and the oracle,
/// clamped to `[oracle·(1−band), oracle·(1+band)]`.
pub fn compute_mark_price(
    bid_price: u64,
    ask_price: u64,
    oracle: u64,
    band_bps: u16,
) -> Result<u64, MathError> {
    if oracle == 0 {
        return Err(MathError::InvalidPrice);
    }

    let candidate: u128 = match (bid_price > 0, ask_price > 0) {
        (true, true) => ((bid_price as u128) + (ask_price as u128)) / 2,
        (true, false) => bid_price as u128,
        (false, true) => ask_price as u128,
        (false, false) => oracle as u128,
    };

    let band = band_bps as u128;
    let lower = (oracle as u128) * (10_000 - band) / 10_000;
    let upper = (oracle as u128) * (10_000 + band) / 10_000;
    let clamped = candidate.clamp(lower, upper);

    u64::try_from(clamped).map_err(|_| MathError::Overflow)
}

/// Move `current` toward `target` by at most `cap_bps · elapsed_slots` of
/// `current` (the per-slot meltdown brake). `cap_bps == 0` disables it; a zero
/// `current` bootstraps to `target`. Rounds down.
pub fn clamp_price_step(current: u64, target: u64, cap_bps: u16, elapsed_slots: u64) -> u64 {
    if current == 0 || cap_bps == 0 {
        return target;
    }
    let max_move = (current as u128)
        .saturating_mul(cap_bps as u128)
        .saturating_mul(elapsed_slots as u128)
        / 10_000;
    let max_move = u64::try_from(max_move).unwrap_or(u64::MAX);
    if target > current {
        current.saturating_add((target - current).min(max_move))
    } else {
        current.saturating_sub((current - target).min(max_move))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midpoint_and_band() {
        assert_eq!(compute_mark_price(100, 110, 104, 500).unwrap(), 105);
        assert_eq!(compute_mark_price(100, 0, 100, 500).unwrap(), 100);
        assert_eq!(compute_mark_price(0, 0, 250, 500).unwrap(), 250);
        assert_eq!(compute_mark_price(190, 210, 100, 500).unwrap(), 105);
        assert_eq!(compute_mark_price(40, 60, 100, 500).unwrap(), 95);
        assert_eq!(
            compute_mark_price(100, 100, 0, 500),
            Err(MathError::InvalidPrice)
        );
    }

    #[test]
    fn test_clamp_price_step() {
        assert_eq!(clamp_price_step(0, 130, 500, 1), 130);
        assert_eq!(clamp_price_step(100, 130, 0, 5), 130);
        assert_eq!(clamp_price_step(100, 130, 500, 1), 105);
        assert_eq!(clamp_price_step(100, 130, 500, 2), 110);
        assert_eq!(clamp_price_step(100, 103, 500, 1), 103);
        assert_eq!(clamp_price_step(100, 50, 500, 1), 95);
        assert_eq!(clamp_price_step(100, 130, 500, 0), 100);
    }

    #[test]
    fn fuzz_mark_within_band_and_step_capped() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..20_000 {
            let bid = next() % 1_000_000;
            let ask = next() % 1_000_000;
            let oracle = 1 + next() % 1_000_000;
            let band = (next() % 5_001) as u16;
            let m = compute_mark_price(bid, ask, oracle, band).unwrap() as u128;
            let lower = (oracle as u128) * (10_000 - band as u128) / 10_000;
            let upper = (oracle as u128) * (10_000 + band as u128) / 10_000;
            assert!(m >= lower && m <= upper);

            let current = 1 + next() % 1_000_000;
            let target = 1 + next() % 1_000_000;
            let cap = (next() % 2_001) as u16;
            let elapsed = next() % 10;
            let stepped = clamp_price_step(current, target, cap, elapsed);
            if cap != 0 {
                let max_move = (current as u128) * (cap as u128) * (elapsed as u128) / 10_000;
                let moved = (stepped as i128 - current as i128).unsigned_abs();
                assert!(moved <= max_move);
            }
        }
    }
}
