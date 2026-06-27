//! Funding-rate math for the batch perp (system-design §9.2).
//!
//! Pure, no-float, signed `i128` fixed-point. Funding keeps the perp price
//! glued to the index: when the mark trades above the oracle, longs pay shorts,
//! and vice-versa. Time is discrete here (one accrual per auction period), so we
//! accrue `(mark - oracle)/oracle`, scaled by the period length and clamped to a
//! per-period cap, into a monotonic **funding index** that each position reads.
//!
//! A position settles funding by paying the delta between the current index and
//! the index it last saw, proportional to its signed size.
//!
//! 🔴 Stability is unproven (system-design §9.2): batch perps may oscillate or
//! be boundary-gamed. These formulas are a reference to **simulate** before
//! mainnet, not a finished design. The cap is the primary safety rail.

use pinocchio::error::ProgramError;

use crate::errors::TempoProgramError;

/// Funding-rate fixed-point scale: rates and the index are in units of `1e-9`
/// (nano-fractions of notional), giving fine granularity without floats.
pub const FUNDING_SCALE: i128 = 1_000_000_000;

/// The per-period funding rate, in `FUNDING_SCALE` units, from the mark/oracle
/// gap scaled by `period_fraction_bps` (period length as a fraction of the
/// funding interval, in bps) and clamped to `±max_rate`.
pub fn period_funding_rate(
    mark: u64,
    oracle: u64,
    period_fraction_bps: u32,
    max_rate: i128,
) -> Result<i128, ProgramError> {
    if oracle == 0 {
        return Err(TempoProgramError::InvalidPrice.into());
    }
    // gap = (mark - oracle) / oracle, in FUNDING_SCALE units.
    let diff = (mark as i128) - (oracle as i128);
    let gap = diff
        .checked_mul(FUNDING_SCALE)
        .ok_or(TempoProgramError::MathOverflow)?
        / (oracle as i128);
    // scale by the period fraction (bps).
    let scaled = gap
        .checked_mul(period_fraction_bps as i128)
        .ok_or(TempoProgramError::MathOverflow)?
        / 10_000;
    Ok(scaled.clamp(-max_rate, max_rate))
}

/// Advance a monotonic funding index by one period's rate (saturating).
pub fn next_funding_index(current_index: i128, period_rate: i128) -> i128 {
    current_index.saturating_add(period_rate)
}

/// Funding owed by a position since it last settled. Positive = the position
/// pays; negative = the position receives. `size_signed` is +long / -short in
/// base units; the payment is `size * (index_now - index_last) / FUNDING_SCALE`,
/// floored toward zero.
pub fn funding_payment(
    size_signed: i128,
    index_now: i128,
    index_last: i128,
) -> Result<i128, ProgramError> {
    let delta = index_now
        .checked_sub(index_last)
        .ok_or(TempoProgramError::MathOverflow)?;
    let num = size_signed
        .checked_mul(delta)
        .ok_or(TempoProgramError::MathOverflow)?;
    Ok(num / FUNDING_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positive_gap_longs_pay() {
        // mark 101, oracle 100 => gap +1% = +0.01 * FUNDING_SCALE = 10_000_000,
        // full period (10_000 bps) uncapped.
        let rate = period_funding_rate(101, 100, 10_000, FUNDING_SCALE).unwrap();
        assert_eq!(rate, FUNDING_SCALE / 100);
        // a long of size 1000 pays size * rate / SCALE = 1000 * 0.01 = 10.
        let pay = funding_payment(1000, rate, 0).unwrap();
        assert_eq!(pay, 10);
        // a short of the same size receives 10.
        assert_eq!(funding_payment(-1000, rate, 0).unwrap(), -10);
    }

    #[test]
    fn test_period_fraction_scales() {
        // half a period (5000 bps) halves the rate.
        let full = period_funding_rate(102, 100, 10_000, FUNDING_SCALE).unwrap();
        let half = period_funding_rate(102, 100, 5_000, FUNDING_SCALE).unwrap();
        assert_eq!(half, full / 2);
    }

    #[test]
    fn test_rate_is_capped() {
        // huge gap, but cap at 0.05% = FUNDING_SCALE/2000.
        let cap = FUNDING_SCALE / 2000;
        let rate = period_funding_rate(200, 100, 10_000, cap).unwrap();
        assert_eq!(rate, cap);
        let rate_neg = period_funding_rate(50, 100, 10_000, cap).unwrap();
        assert_eq!(rate_neg, -cap);
    }

    #[test]
    fn test_index_accrues_and_payment_uses_delta() {
        let r = period_funding_rate(101, 100, 10_000, FUNDING_SCALE).unwrap();
        let i0 = 0i128;
        let i1 = next_funding_index(i0, r);
        let i2 = next_funding_index(i1, r);
        // a position opened at i1 paying at i2 owes one period only.
        assert_eq!(funding_payment(1000, i2, i1).unwrap(), 10);
        // opened at i0 paying at i2 owes two periods.
        assert_eq!(funding_payment(1000, i2, i0).unwrap(), 20);
    }

    #[test]
    fn test_zero_oracle_rejected() {
        assert_eq!(
            period_funding_rate(100, 0, 10_000, FUNDING_SCALE),
            Err(TempoProgramError::InvalidPrice.into())
        );
    }

    /// Property fuzz: the period funding rate is always within `±max_rate`
    /// and sign-consistent with the mark/oracle gap.
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
            assert!(rate >= -cap && rate <= cap, "rate {rate} outside ±{cap}");
            if mark > oracle && frac > 0 {
                assert!(rate >= 0, "mark>oracle → longs pay (rate >= 0)");
            }
            if mark < oracle && frac > 0 {
                assert!(rate <= 0, "mark<oracle → shorts pay (rate <= 0)");
            }
        }
    }
}
