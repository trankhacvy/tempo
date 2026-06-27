//! Funding-rate math. Mirror of `program/src/funding.rs`. Signed i128 fixed-point.

use crate::error::MathError;

/// Funding-rate fixed-point scale (`1e-9` units).
pub const FUNDING_SCALE: i128 = 1_000_000_000;

/// The per-period funding rate, in `FUNDING_SCALE` units, from the mark/oracle
/// gap scaled by `period_fraction_bps` and clamped to `±max_rate`.
pub fn period_funding_rate(
    mark: u64,
    oracle: u64,
    period_fraction_bps: u32,
    max_rate: i128,
) -> Result<i128, MathError> {
    if oracle == 0 {
        return Err(MathError::InvalidPrice);
    }
    let diff = (mark as i128) - (oracle as i128);
    let gap = diff.checked_mul(FUNDING_SCALE).ok_or(MathError::Overflow)? / (oracle as i128);
    let scaled = gap
        .checked_mul(period_fraction_bps as i128)
        .ok_or(MathError::Overflow)?
        / 10_000;
    Ok(scaled.clamp(-max_rate, max_rate))
}

/// Advance a monotonic funding index by one period's rate (saturating).
pub fn next_funding_index(current_index: i128, period_rate: i128) -> i128 {
    current_index.saturating_add(period_rate)
}

/// Funding owed by a position since it last settled. Positive = the position pays.
pub fn funding_payment(
    size_signed: i128,
    index_now: i128,
    index_last: i128,
) -> Result<i128, MathError> {
    let delta = index_now
        .checked_sub(index_last)
        .ok_or(MathError::Overflow)?;
    let num = size_signed.checked_mul(delta).ok_or(MathError::Overflow)?;
    Ok(num / FUNDING_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positive_gap_longs_pay() {
        let rate = period_funding_rate(101, 100, 10_000, FUNDING_SCALE).unwrap();
        assert_eq!(rate, FUNDING_SCALE / 100);
        assert_eq!(funding_payment(1000, rate, 0).unwrap(), 10);
        assert_eq!(funding_payment(-1000, rate, 0).unwrap(), -10);
    }

    #[test]
    fn test_rate_is_capped() {
        let cap = FUNDING_SCALE / 2000;
        assert_eq!(period_funding_rate(200, 100, 10_000, cap).unwrap(), cap);
        assert_eq!(period_funding_rate(50, 100, 10_000, cap).unwrap(), -cap);
    }

    #[test]
    fn test_zero_oracle_rejected() {
        assert_eq!(
            period_funding_rate(100, 0, 10_000, FUNDING_SCALE),
            Err(MathError::InvalidPrice)
        );
    }

    #[test]
    fn fuzz_period_rate_clamped_and_signed() {
        let mut seed: u64 = 0x14057B7E_F767814F;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..20_000 {
            let mark = 1 + next() % 1_000_000;
            let oracle = 1 + next() % 1_000_000;
            let frac = (next() % 10_001) as u32;
            let cap = 1 + (next() % FUNDING_SCALE as u64) as i128;
            let rate = period_funding_rate(mark, oracle, frac, cap).unwrap();
            assert!(rate >= -cap && rate <= cap);
            if mark > oracle && frac > 0 {
                assert!(rate >= 0);
            }
            if mark < oracle && frac > 0 {
                assert!(rate <= 0);
            }
        }
    }
}
