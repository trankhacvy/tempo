//! The pure liquidation decision core: candidates → actions, no RPC, no clock.
//! Both gates call `tempo_math::margin` — the same arithmetic the program's
//! `liquidate` / `liquidate_cross` enforce — so a local "liquidatable" the program
//! rejects can only be a race (handled as benign), never a logic drift.

use solana_sdk::pubkey::Pubkey;

use tempo_math::margin::{equity, is_liquidatable, maintenance_margin, unrealized_pnl};
use tempo_sdk::ix::CrossLeg;

use crate::snapshot::{Candidate, CrossMember};

/// A liquidation the scan will fire.
#[derive(Clone, Debug)]
pub enum LiqAction {
    Isolated {
        position: Pubkey,
        owner: Pubkey,
        market: Pubkey,
        oracle: Pubkey,
    },
    Cross {
        owner: Pubkey,
        legs: Vec<CrossLeg>,
    },
}

/// Does this isolated position breach maintenance at its raw mark? The same
/// threshold the program's `liquidate` enforces (`equity < maintenance`).
pub fn isolated_liquidatable(c: &Candidate) -> bool {
    let size = c.view.size as i128;
    let eq = equity(
        c.view.collateral,
        c.view.realized_pnl,
        unrealized_pnl(size, c.view.entry_price, c.mark),
    );
    let maint = maintenance_margin(size, c.mark, c.maintenance_bps);
    is_liquidatable(eq, maint)
}

/// Combined-health gate for a cross account, mirroring `liquidate_cross`: one
/// combined equity vs one combined maintenance over EVERY member (each live leg
/// priced with its own market's bps). Returns the member legs to feed the SDK
/// builder (live triples + flat bare positions, in member order) when the account
/// is underwater and has a live close target, else `None`.
pub fn cross_liquidatable(balance: u64, members: &[CrossMember]) -> Option<Vec<CrossLeg>> {
    let mut combined_eq: i128 = balance as i128;
    let mut combined_maint: i128 = 0;
    let mut legs = Vec::with_capacity(members.len());
    let mut has_live_target = false;

    for m in members {
        if m.size == 0 {
            combined_eq = combined_eq.saturating_add(m.realized_pnl);
            legs.push(CrossLeg::Flat {
                position: m.position,
            });
            continue;
        }
        let size = m.size as i128;
        combined_eq = combined_eq
            .saturating_add(m.realized_pnl)
            .saturating_add(unrealized_pnl(size, m.entry_price, m.mark));
        combined_maint =
            combined_maint.saturating_add(maintenance_margin(size, m.mark, m.maintenance_bps));
        legs.push(CrossLeg::Live {
            position: m.position,
            market: m.market,
            oracle: m.oracle,
        });
        has_live_target = true;
    }

    if has_live_target && is_liquidatable(combined_eq, combined_maint) {
        Some(legs)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempo_sdk::accounts::PositionView;

    fn isolated(collateral: u64, size: i64, entry: u64, mark: u64) -> Candidate {
        Candidate {
            key: Pubkey::new_unique(),
            view: PositionView {
                owner: Pubkey::new_unique(),
                market: Pubkey::new_unique(),
                size,
                entry_price: entry,
                collateral,
                realized_pnl: 0,
                margin_mode: 0,
            },
            market: Pubkey::new_unique(),
            oracle: Pubkey::new_unique(),
            mark,
            maintenance_bps: 500,
        }
    }

    fn member(size: i64, entry: u64, mark: u64, realized: i128) -> CrossMember {
        CrossMember {
            position: Pubkey::new_unique(),
            size,
            entry_price: entry,
            realized_pnl: realized,
            market: Pubkey::new_unique(),
            oracle: Pubkey::new_unique(),
            mark,
            maintenance_bps: 500,
        }
    }

    #[test]
    fn isolated_underwater_fires() {
        // long 10 @ 100, collateral 50, mark 80 → unrl -200, eq -150 < maint 40.
        assert!(isolated_liquidatable(&isolated(50, 10, 100, 80)));
    }

    #[test]
    fn isolated_healthy_does_not_fire() {
        // long 10 @ 100, collateral 200, mark 110 → eq 300 > maint 55.
        assert!(!isolated_liquidatable(&isolated(200, 10, 100, 110)));
    }

    #[test]
    fn cross_winner_rescues_loser_stays_healthy() {
        // losing leg -200, winning leg +300, balance 100 → eq 200 > maint 105.
        let members = [member(10, 100, 80, 0), member(10, 100, 130, 0)];
        assert!(cross_liquidatable(100, &members).is_none());
    }

    #[test]
    fn cross_two_losers_breach() {
        // two legs (10 @ 100, mark 96) → -40 each; balance 89 → eq 9 < maint 96.
        let members = [member(10, 100, 96, 0), member(10, 100, 96, 0)];
        let legs = cross_liquidatable(89, &members).expect("liquidatable");
        assert_eq!(legs.len(), 2);
        assert!(matches!(legs[0], CrossLeg::Live { .. }));
    }

    #[test]
    fn all_flat_group_has_no_target() {
        // flat member with a loss → equity negative, but nothing live to close.
        let flat = CrossMember {
            position: Pubkey::new_unique(),
            size: 0,
            entry_price: 0,
            realized_pnl: -100,
            market: Pubkey::default(),
            oracle: Pubkey::default(),
            mark: 0,
            maintenance_bps: 0,
        };
        assert!(cross_liquidatable(0, &[flat]).is_none());
    }
}
