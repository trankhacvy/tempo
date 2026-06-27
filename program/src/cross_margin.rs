//! Cross-margin risk math (account-free and unit-tested).
//!
//! Today each `Position` is margined in isolation. Cross-margin nets risk across
//! all of one account's positions: profit on one leg offsets loss on another, so
//! an account is judged by ONE combined equity vs ONE combined maintenance
//! requirement. These pure functions take the per-leg `(size, entry, mark)` the
//! caller passes in (one per market the account holds) and compute that combined
//! view. The on-chain wrapper supplies the legs from the account's
//! member positions + each market's fresh effective price.
//!
//! No floats; same integer rounding as `margin.rs` (it is reused directly).

use crate::margin::{maintenance_margin, unrealized_pnl};

/// One position's risk inputs for the combined view: signed `size` (+long /
/// −short), the VWAP `entry`, and the market's current risk `mark`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leg {
    pub size: i128,
    pub entry: u64,
    pub mark: u64,
}

/// One member leg's contribution to a combined cross-margin health view: how much
/// it adds to combined equity and to the combined maintenance requirement. This is
/// the single per-leg primitive the on-chain `withdraw_cross` / `liquidate_cross`
/// gates run, so the unit-tested math here is exactly the executed math (it used to
/// be hand-inlined and could drift — known-issues §2.9b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegContribution {
    /// `realized` + recognized unrealized PnL − `pending` (funding + social loss).
    pub equity: i128,
    /// Maintenance margin required for this leg (`|size|·mark·bps/10_000`).
    pub maintenance: i128,
}

impl LegContribution {
    /// A flat (`size == 0`) member: it can only carry stored `realized` PnL — zero
    /// unrealized, zero maintenance, zero unsettled funding/social — so it needs no
    /// market or oracle (known-issues §2.4).
    #[inline(always)]
    pub fn flat(realized: i128) -> Self {
        Self {
            equity: realized,
            maintenance: 0,
        }
    }
}

/// Compute one *live* leg's contribution to the combined view.
///
/// `credit_unrealized_gains` is the one knob that distinguishes the two callers:
/// - **liquidation** marks to the true price — both gains and losses count toward
///   whether the account is underwater (`true`).
/// - **withdrawal** applies the backed-profit rule — only *losses* dock equity;
///   unbacked paper gains are never credited toward what may be pulled out (`false`).
///
/// `pending` is funding + socialized loss accrued on the leg but not yet settled
/// (docked so a read-only leg's debt can't be withdrawn/liquidated around — §1.4).
pub fn leg_contribution(
    leg: Leg,
    bps: u16,
    realized: i128,
    pending: i128,
    credit_unrealized_gains: bool,
) -> LegContribution {
    let unrealized = unrealized_pnl(leg.size, leg.entry, leg.mark);
    let recognized = if credit_unrealized_gains {
        unrealized
    } else {
        unrealized.min(0)
    };
    LegContribution {
        equity: realized.saturating_add(recognized).saturating_sub(pending),
        maintenance: maintenance_margin(leg.size, leg.mark, bps),
    }
}

/// Combined account equity = posted `collateral` + netted `realized` PnL across
/// the account + Σ unrealized PnL over every leg. (Reference model: full
/// mark-to-market, no pending term — the on-chain gates add per-leg pending/realized
/// via [`leg_contribution`].)
pub fn account_equity(collateral: u64, realized: i128, legs: &[Leg]) -> i128 {
    legs.iter().fold(collateral as i128 + realized, |eq, l| {
        eq.saturating_add(leg_contribution(*l, 0, 0, 0, true).equity)
    })
}

/// Combined maintenance requirement = Σ `|size|·mark·bps/10_000` over every leg.
pub fn account_maintenance(legs: &[Leg], bps: u16) -> i128 {
    legs.iter().fold(0i128, |m, l| {
        m.saturating_add(leg_contribution(*l, bps, 0, 0, true).maintenance)
    })
}

/// An account is liquidatable when its combined equity falls below its combined
/// maintenance requirement.
pub fn account_liquidatable(legs: &[Leg], collateral: u64, realized: i128, bps: u16) -> bool {
    account_equity(collateral, realized, legs) < account_maintenance(legs, bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(size: i128, entry: u64, mark: u64) -> Leg {
        Leg { size, entry, mark }
    }

    #[test]
    fn test_profit_offsets_loss_across_legs() {
        // Long A: +10 @ 30, mark 40 → +100. Short B: -10 @ 20, mark 25 → -50.
        let legs = [leg(10, 30, 40), leg(-10, 20, 25)];
        // collateral 100, no realized: equity = 100 + 100 - 50 = 150.
        assert_eq!(account_equity(100, 0, &legs), 150);
        // maintenance = 10*40*5% + 10*25*5% = 20 + 12.5→12 = 32 (floored per leg).
        assert_eq!(account_maintenance(&legs, 500), 20 + 12);
        assert!(!account_liquidatable(&legs, 100, 0, 500));
    }

    #[test]
    fn test_isolated_liquidatable_but_cross_healthy() {
        // The losing leg alone would be underwater, but the winning leg rescues it.
        let losing = leg(10, 100, 80); // -200 unrealized
        let winning = leg(10, 100, 130); // +300 unrealized
                                         // Isolated: collateral 50 on the losing leg → equity 50-200 = -150 < maint.
        assert!(account_liquidatable(&[losing], 50, 0, 500));
        // Cross-margined with the winner: equity = 100 + (-200) + 300 = 200 > maint.
        assert!(!account_liquidatable(&[losing, winning], 100, 0, 500));
    }

    #[test]
    fn test_cross_can_be_liquidatable_when_isolated_is_not() {
        // Two equal losing legs: each needs its own maintenance, so holding both
        // requires more margin than holding one. unrealized = 10*(96-100) = -40 each;
        // per-leg maintenance = 10*96*5% = 48.
        let a = leg(10, 100, 96);
        let b = leg(10, 100, 96);
        // collateral 89: leg a alone → equity 89-40 = 49 ≥ maint 48 → healthy.
        assert!(!account_liquidatable(&[a], 89, 0, 500));
        // The pair → equity 89-80 = 9 < maint 96 → liquidatable.
        assert!(account_liquidatable(&[a, b], 89, 0, 500));
    }

    #[test]
    fn test_leg_contribution_credit_modes_and_pending() {
        // Winning long: +10 @ 100, mark 130 → +300 unrealized; realized +5; pending 7.
        let winner = leg(10, 100, 130);
        // Liquidation marks to true price → full +300 counts.
        let liq = leg_contribution(winner, 500, 5, 7, true);
        assert_eq!(liq.equity, 5 + 300 - 7);
        assert_eq!(liq.maintenance, maintenance_margin(10, 130, 500));
        // Withdrawal applies the backed-profit rule → unrealized GAIN (+300) is
        // dropped; only realized (+5) and pending (−7) remain.
        let wd = leg_contribution(winner, 500, 5, 7, false);
        assert_eq!(wd.equity, 5 - 7);
        assert_eq!(
            wd.maintenance, liq.maintenance,
            "maintenance is mode-agnostic"
        );

        // Losing long: +10 @ 100, mark 80 → −200 unrealized; both modes count losses.
        let loser = leg(10, 100, 80);
        assert_eq!(leg_contribution(loser, 500, 0, 0, true).equity, -200);
        assert_eq!(leg_contribution(loser, 500, 0, 0, false).equity, -200);

        // Flat helper: only realized, no maintenance.
        assert_eq!(
            LegContribution::flat(42),
            LegContribution {
                equity: 42,
                maintenance: 0
            }
        );
    }

    #[test]
    fn fuzz_equity_is_sum_of_legs() {
        // account_equity == collateral + realized + Σ per-leg unrealized.
        let mut seed: u64 = 0xCAFE_F00D_1234_5678;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        for _ in 0..20_000 {
            let collateral = next() % 1_000_000;
            let realized = (next() % 2_000_000) as i128 - 1_000_000;
            let n = (next() % 8) as usize;
            let mut legs = alloc::vec::Vec::new();
            let mut expect = collateral as i128 + realized;
            for _ in 0..n {
                let size = (next() % 200_000) as i128 - 100_000;
                let entry = 1 + next() % 100_000;
                let mark = 1 + next() % 100_000;
                legs.push(leg(size, entry, mark));
                expect = expect.saturating_add(unrealized_pnl(size, entry, mark));
            }
            assert_eq!(account_equity(collateral, realized, &legs), expect);
        }
    }
}
