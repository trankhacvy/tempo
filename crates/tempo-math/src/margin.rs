//! Margin + liquidation math. Mirror of `program/src/margin.rs`.
//!
//! Unit assumption: `collateral`, `realized_pnl`, and `size · price` share one
//! base unit. No floats; margins round up, payouts round down.

use crate::wide::{mul_div_ceil, mul_div_floor};

/// Unrealized PnL of a signed position marked at `mark`.
pub fn unrealized_pnl(size_signed: i128, entry: u64, mark: u64) -> i128 {
    size_signed * (mark as i128 - entry as i128)
}

/// Account equity = posted collateral + realized PnL + unrealized PnL.
pub fn equity(collateral: u64, realized_pnl: i128, unrealized: i128) -> i128 {
    collateral as i128 + realized_pnl + unrealized
}

/// Maintenance margin = `|size| · mark · maintenance_bps / 10_000` (floored).
pub fn maintenance_margin(size_signed: i128, mark: u64, maintenance_bps: u16) -> i128 {
    let notional = size_signed.unsigned_abs().saturating_mul(mark as u128);
    mul_div_floor(notional, maintenance_bps as u128, 10_000)
        .and_then(|m| i128::try_from(m).ok())
        .unwrap_or(i128::MAX)
}

/// Initial margin to lock when opening/increasing, priced at `entry`. Rounds up.
pub fn initial_margin(size_added: u64, entry: u64, initial_bps: u16) -> u64 {
    let notional = (size_added as u128) * (entry as u128);
    mul_div_ceil(notional, initial_bps as u128, 10_000)
        .and_then(|m| u64::try_from(m).ok())
        .unwrap_or(u64::MAX)
}

/// A position is liquidatable when its equity falls below maintenance margin.
pub fn is_liquidatable(equity: i128, maintenance: i128) -> bool {
    equity < maintenance
}

/// How far a position is below maintenance: `max(0, maintenance − equity)`.
pub fn maintenance_deficit(equity: i128, maintenance: i128) -> u128 {
    if equity >= maintenance {
        0
    } else {
        (maintenance - equity).unsigned_abs()
    }
}

/// Protocol fee on a settled fill = `qty · price · fee_bps / 10_000` (floored).
pub fn protocol_fee(qty: u64, price: u64, fee_bps: u16) -> u64 {
    let notional = (qty as u128) * (price as u128);
    mul_div_floor(notional, fee_bps as u128, 10_000)
        .and_then(|m| u64::try_from(m).ok())
        .unwrap_or(u64::MAX)
}

/// Signed protocol fee; a negative `fee_bps` yields a rebate (negative result).
pub fn signed_protocol_fee(qty: u64, price: u64, fee_bps: i16) -> i128 {
    let notional = (qty as u128) * (price as u128);
    let mag = mul_div_floor(notional, fee_bps.unsigned_abs() as u128, 10_000)
        .and_then(|m| i128::try_from(m).ok())
        .unwrap_or(i128::MAX);
    if fee_bps < 0 {
        -mag
    } else {
        mag
    }
}

/// Outcome of closing a position at `mark` during liquidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiquidationOutcome {
    pub equity: i128,
    pub penalty: u64,
    pub returned_to_owner: u64,
    pub bad_debt: u64,
}

/// Compute the settlement of a liquidated position.
pub fn liquidation_outcome(
    collateral: u64,
    realized_pnl: i128,
    size_signed: i128,
    entry: u64,
    mark: u64,
    penalty_bps: u16,
) -> LiquidationOutcome {
    let eq = equity(
        collateral,
        realized_pnl,
        unrealized_pnl(size_signed, entry, mark),
    );
    if eq <= 0 {
        let bad = (-eq).min(i128::from(u64::MAX)) as u64;
        return LiquidationOutcome {
            equity: eq,
            penalty: 0,
            returned_to_owner: 0,
            bad_debt: bad,
        };
    }
    let eq_u = eq.min(i128::from(u64::MAX)) as u64;
    let notional = size_signed.unsigned_abs().saturating_mul(mark as u128);
    let penalty = u64::try_from(notional * (penalty_bps as u128) / 10_000)
        .unwrap_or(u64::MAX)
        .min(eq_u);
    LiquidationOutcome {
        equity: eq,
        penalty,
        returned_to_owner: eq_u - penalty,
        bad_debt: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unrealized_and_equity() {
        assert_eq!(unrealized_pnl(10, 100, 120), 200);
        assert_eq!(unrealized_pnl(-10, 100, 120), -200);
        assert_eq!(equity(1000, 50, 200), 1250);
    }

    #[test]
    fn test_maintenance_and_liquidatable() {
        let maint = maintenance_margin(10, 120, 500);
        assert_eq!(maint, 60);
        assert!(!is_liquidatable(100, maint));
        assert!(is_liquidatable(59, maint));
    }

    #[test]
    fn test_initial_margin() {
        assert_eq!(initial_margin(10, 100, 500), 50);
        assert_eq!(initial_margin(7, 33, 30), 1);
    }

    #[test]
    fn test_liquidation_solvent_and_bad_debt() {
        let o = liquidation_outcome(200, 0, 10, 100, 110, 100);
        assert_eq!(o.equity, 300);
        assert_eq!(o.penalty, 11);
        assert_eq!(o.returned_to_owner, 289);
        assert_eq!(o.bad_debt, 0);

        let o = liquidation_outcome(50, 0, 10, 100, 80, 100);
        assert_eq!(o.equity, -150);
        assert_eq!(o.bad_debt, 150);
    }

    #[test]
    fn fuzz_liquidation_outcome_conserves() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..20_000 {
            let collateral = next() % 1_000_000;
            let realized = (next() % 2_000_000) as i128 - 1_000_000;
            let size = (next() % 200_000) as i128 - 100_000;
            let entry = 1 + next() % 100_000;
            let mark = 1 + next() % 100_000;
            let penalty_bps = (next() % 10_001) as u16;
            let o = liquidation_outcome(collateral, realized, size, entry, mark, penalty_bps);
            if o.equity <= 0 {
                assert_eq!(o.penalty, 0);
                assert_eq!(o.returned_to_owner, 0);
                assert_eq!(o.bad_debt as i128, -o.equity);
            } else {
                assert_eq!(o.bad_debt, 0);
                let eq_u = o.equity.min(i128::from(u64::MAX)) as u64;
                assert!(o.penalty <= eq_u);
                assert_eq!(o.returned_to_owner + o.penalty, eq_u);
            }
        }
    }
}
