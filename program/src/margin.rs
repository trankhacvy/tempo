//! Margin + liquidation math (system-design §9.3).
//!
//! Pure, no-float, signed `i128`. **⚠️ Unit assumption:** every monetary
//! quantity — `collateral`, `realized_pnl`, and `size · price` — shares one base
//! unit. An operator achieves this by choosing the market `tick_size` and
//! contract size so a position's notional (`|size| · price`) is denominated in
//! the collateral mint's base units. Reconciling these units rigorously against
//! the oracle's 1e8 scale is a later enhancement.

/// Unrealized PnL of a signed position marked at `mark` (long gains as mark
/// rises). `entry` is the VWAP entry price.
pub fn unrealized_pnl(size_signed: i128, entry: u64, mark: u64) -> i128 {
    size_signed * (mark as i128 - entry as i128)
}

/// Account equity = posted collateral + realized PnL + unrealized PnL.
pub fn equity(collateral: u64, realized_pnl: i128, unrealized: i128) -> i128 {
    collateral as i128 + realized_pnl + unrealized
}

/// Maintenance margin = `|size| · mark · maintenance_bps / 10_000` (notional-based).
/// Uses 256-bit-intermediate `mul_div` so `notional · bps` cannot overflow.
pub fn maintenance_margin(size_signed: i128, mark: u64, maintenance_bps: u16) -> i128 {
    let notional = size_signed.unsigned_abs().saturating_mul(mark as u128);
    crate::wide_math::mul_div_floor(notional, maintenance_bps as u128, 10_000)
        .and_then(|m| i128::try_from(m).ok())
        .unwrap_or(i128::MAX)
}

/// Initial margin to lock when opening/increasing, priced at `entry` (u64,
/// saturating). The caller passes the market's `initial_margin_bps` — the buffer at
/// or above maintenance (missing-features §1.2) — so a position never opens exactly
/// on its own liquidation line. (A pre-v8 / no-money-path market reports `0` and
/// falls back to maintenance via `Market::initial_margin_bps`.)
pub fn initial_margin(size_added: u64, entry: u64, initial_bps: u16) -> u64 {
    let notional = (size_added as u128) * (entry as u128);
    // Round the requirement UP — never lock less than policy (round against the
    // user); flooring could lock 0 for a small notional. 256-bit `mul_div` keeps
    // `notional · bps` from overflowing.
    crate::wide_math::mul_div_ceil(notional, initial_bps as u128, 10_000)
        .and_then(|m| u64::try_from(m).ok())
        .unwrap_or(u64::MAX)
}

/// Worst-case standing margin for a maker ladder (missing-features §7.1):
/// `initial_margin(Σ all level sizes, window_top)`. Deliberately
/// **mid-independent** — every level (both sides) is priced at the window top (a
/// bid buys at ≤ its limit ≤ the top; an ask's worst in-window short mark is the
/// top), so the reservation is a pure function of the ladder SHAPE. Moving
/// `mid_tick` never changes it, keeping the O(1) `update_maker_quote_mid`
/// re-quote path collateral-free. Rounds UP via `initial_margin` (never lock
/// less than policy). Both auctions can cross in the same round, so both sides
/// reserve simultaneously (the sum, not the max).
pub fn ladder_reservation(total_ladder_qty: u64, window_top_price: u64, initial_bps: u16) -> u64 {
    initial_margin(total_ladder_qty, window_top_price, initial_bps)
}

/// A position is liquidatable when its equity falls below maintenance margin.
pub fn is_liquidatable(equity: i128, maintenance: i128) -> bool {
    equity < maintenance
}

/// How far a position is below its maintenance requirement (progress metric):
/// `max(0, maintenance − equity)` as an unsigned amount. Zero when healthy. A
/// liquidation step must strictly shrink this (or flatten the position) to count
/// as progress.
pub fn maintenance_deficit(equity: i128, maintenance: i128) -> u128 {
    if equity >= maintenance {
        0
    } else {
        (maintenance - equity).unsigned_abs()
    }
}

/// Protocol fee on a settled fill = `qty · price · fee_bps / 10_000` (saturating).
pub fn protocol_fee(qty: u64, price: u64, fee_bps: u16) -> u64 {
    let notional = (qty as u128) * (price as u128);
    crate::wide_math::mul_div_floor(notional, fee_bps as u128, 10_000)
        .and_then(|m| u64::try_from(m).ok())
        .unwrap_or(u64::MAX)
}

/// Signed protocol fee = `qty · price · fee_bps / 10_000`; a negative `fee_bps`
/// yields a negative result (a rebate owed to the trader).
pub fn signed_protocol_fee(qty: u64, price: u64, fee_bps: i16) -> i128 {
    let notional = (qty as u128) * (price as u128);
    let mag = crate::wide_math::mul_div_floor(notional, fee_bps.unsigned_abs() as u128, 10_000)
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
    /// Final equity at the close mark (may be negative → bad debt).
    pub equity: i128,
    /// Penalty paid to the liquidator (capped at non-negative equity).
    pub penalty: u64,
    /// Collateral returned to the owner after the penalty (0 if wiped out).
    pub returned_to_owner: u64,
    /// Shortfall the insurance fund must cover (only when equity < 0).
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
    let notional = size_signed.unsigned_abs() * (mark as u128);
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
        // long 10 @ entry 100, mark 120 → +200 unrealized.
        assert_eq!(unrealized_pnl(10, 100, 120), 200);
        // short 10 @ entry 100, mark 120 → -200.
        assert_eq!(unrealized_pnl(-10, 100, 120), -200);
        assert_eq!(equity(1000, 50, 200), 1250);
    }

    #[test]
    fn test_maintenance_and_liquidatable() {
        // |size|=10, mark=120, 5% → notional 1200, maint 60.
        let maint = maintenance_margin(10, 120, 500);
        assert_eq!(maint, 60);
        assert!(!is_liquidatable(100, maint));
        assert!(is_liquidatable(59, maint));
    }

    #[test]
    fn test_maintenance_deficit() {
        assert_eq!(maintenance_deficit(100, 60), 0, "healthy → no deficit");
        assert_eq!(maintenance_deficit(60, 60), 0, "at the line → no deficit");
        assert_eq!(maintenance_deficit(50, 60), 10, "below by 10");
        assert_eq!(maintenance_deficit(-5, 60), 65, "negative equity widens it");
    }

    #[test]
    fn test_liquidation_solvent() {
        // long 10 @ 100, collateral 200, mark 110 → unrl +100, eq 300; penalty 1% of 1100 = 11.
        let o = liquidation_outcome(200, 0, 10, 100, 110, 100);
        assert_eq!(o.equity, 300);
        assert_eq!(o.penalty, 11);
        assert_eq!(o.returned_to_owner, 289);
        assert_eq!(o.bad_debt, 0);
    }

    #[test]
    fn test_liquidation_bad_debt() {
        // long 10 @ 100, collateral 50, mark 80 → unrl -200, eq -150 → bad debt 150.
        let o = liquidation_outcome(50, 0, 10, 100, 80, 100);
        assert_eq!(o.equity, -150);
        assert_eq!(o.penalty, 0);
        assert_eq!(o.returned_to_owner, 0);
        assert_eq!(o.bad_debt, 150);
    }

    #[test]
    fn test_initial_margin() {
        // open 10 @ 100, 5% → 50 (exact, unchanged by ceil).
        assert_eq!(initial_margin(10, 100, 500), 50);
        // small notional that floored to 0 now rounds up: 7·33·30bps = 6930/10000 → 1.
        assert_eq!(initial_margin(7, 33, 30), 1);
    }

    #[test]
    fn test_ladder_reservation() {
        // 12 bid lots + 8 ask lots, window top 640, 10% initial → 20·640·0.10 = 1280.
        assert_eq!(ladder_reservation(20, 640, 1000), 1280);
        // Empty ladder reserves nothing.
        assert_eq!(ladder_reservation(0, 640, 1000), 0);
        // Rounds UP like initial_margin (never lock less than policy).
        assert_eq!(ladder_reservation(7, 33, 30), 1);
        // Equals the taker-side formula by construction (single source of truth).
        assert_eq!(
            ladder_reservation(20, 640, 1000),
            initial_margin(20, 640, 1000)
        );
    }

    #[test]
    fn test_protocol_fee() {
        // fill 10 @ 100, 30 bps → 1000 * 0.003 = 3.
        assert_eq!(protocol_fee(10, 100, 30), 3);
        assert_eq!(protocol_fee(10, 100, 0), 0);
    }

    /// Property fuzz: `liquidation_outcome` conserves and never over-pays.
    /// Over thousands of random positions: the owner's return plus the liquidator's
    /// penalty never exceed the seized collateral plus realized gain, and a negative
    /// equity yields exactly its magnitude as bad debt with no payouts.
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
                // returned + penalty == equity (capped at u64), penalty never exceeds it.
                let eq_u = o.equity.min(i128::from(u64::MAX)) as u64;
                assert!(o.penalty <= eq_u);
                assert_eq!(o.returned_to_owner + o.penalty, eq_u);
            }
        }
    }

    #[test]
    fn test_signed_protocol_fee() {
        // taker 10 @ 100, +30 bps → +3 (a cost).
        assert_eq!(signed_protocol_fee(10, 100, 30), 3);
        // maker 100 @ 100, -5 bps → -5 (a rebate).
        assert_eq!(signed_protocol_fee(100, 100, -5), -5);
        // small notional rebate floors to 0.
        assert_eq!(signed_protocol_fee(10, 100, -5), 0);
        assert_eq!(signed_protocol_fee(10, 100, 0), 0);
    }
}
