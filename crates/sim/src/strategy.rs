//! The trader's order-generation strategy: a pure function from (market view, own
//! position, free collateral, rng, config) to a set of taker orders. No RPC, no
//! clock, no floats — the same discipline as `mm_bot::strategy::build_quote`.
//!
//! Takers must *cross the spread* to fill. The window is oracle-anchored, so we
//! center on the window mid (`num_ticks/2`) exactly like the maker. A buy lands in
//! `AskDemand` and crosses the maker's asks (which rest at `mid + offset`), so a
//! crossing buy prices at a tick `>= mid + inner_spread`; a crossing sell prices at
//! `<= mid - inner_spread`. A `passive` persona instead rests *inside* the spread.

use tempo_math::margin::initial_margin;
use tempo_math::tick::tick_to_price;

use tempo_sdk::accounts::{MarketView, PositionView};

use crate::persona::{Persona, BUY};
use crate::rng::SimRng;

/// Sentinel free-collateral budget for a clearing-only market (no money path → no
/// on-chain margin reservation). Mirrors `mm_bot`'s `UNMETERED_COLLATERAL`.
pub const UNMETERED_COLLATERAL: u64 = u64::MAX / 2;

/// One taker order to submit this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderIntent {
    pub side: u8,
    pub price: u64,
    pub quantity: u64,
    pub reduce_only: bool,
}

/// The knobs `build_orders` needs (the subset of `SimConfig` the pure builder reads).
#[derive(Debug, Clone)]
pub struct TraderConfig {
    pub persona: Persona,
    pub max_orders: u8,
    pub base_size: u64,
    /// How far past `mid` a crossing order prices (in ticks, on top of the maker's
    /// inner spread so we land beyond the best maker quote).
    pub aggression_ticks: u16,
    /// The maker's inner spread, so our crossing orders clear the best quote.
    pub inner_spread_ticks: u16,
    /// Force every order to a fixed side (`Some(0)` buy / `Some(1)` sell), overriding
    /// the persona. With all traders on one side the maker only ever accumulates the
    /// opposite inventory and never offsets it, so no fill realizes PnL — which keeps
    /// a v1.1-money-path market from wedging on `InsuranceInsolvent` (the insurance
    /// pool is only touched by realized PnL). `None` = persona-driven (two-sided).
    pub force_side: Option<u8>,
}

/// Build this round's taker orders. `free_collateral == UNMETERED_COLLATERAL` on a
/// clearing-only market (no sizing constraint).
pub fn build_orders(
    market: &MarketView,
    position: Option<&PositionView>,
    free_collateral: u64,
    rng: &mut SimRng,
    cfg: &TraderConfig,
) -> Vec<OrderIntent> {
    let num_ticks = market.num_ticks;
    if num_ticks == 0 || free_collateral == 0 {
        return Vec::new();
    }

    let mid = (num_ticks / 2) as i64;
    let inventory = position.map(|p| p.size).unwrap_or(0);
    let plan = cfg.persona.plan_round(inventory, rng, cfg.max_orders);
    let crosses = cfg.persona.crosses();
    let size_mult = cfg.persona.size_mult();
    let bps = market.effective_initial_margin_bps();

    let mut out: Vec<OrderIntent> = Vec::with_capacity(plan.count as usize);
    for _ in 0..plan.count.min(cfg.max_orders) {
        let side = match cfg.force_side {
            Some(s) => s,
            None => plan.next_side(rng),
        };

        // Crossing orders price beyond the maker's inner quote; passive orders rest
        // one tick inside it (so they add depth but do not fill).
        let off = if crosses {
            (cfg.inner_spread_ticks + cfg.aggression_ticks) as i64
        } else {
            (cfg.inner_spread_ticks as i64 - 1).max(0)
        };
        let tick = match side {
            BUY => (mid + off).clamp(0, num_ticks as i64 - 1),
            _ => (mid - off).clamp(0, num_ticks as i64 - 1),
        } as u32;

        let price =
            match tick_to_price(tick, market.window_floor_price, market.tick_size, num_ticks) {
                Ok(p) => p,
                Err(_) => continue,
            };

        let want = cfg
            .base_size
            .saturating_mul(size_mult)
            .saturating_add(rng.range(0, cfg.base_size));
        // A buy's worst-case fill is at its own (crossing) price; a sell can clear no
        // higher than the window top — bound margin by the same worst case the
        // program reserves.
        let worst_price = if side == BUY {
            price
        } else {
            market.window_top_price()
        };
        let qty = cap_by_collateral(want, worst_price, bps, free_collateral);
        if qty == 0 {
            continue;
        }
        out.push(OrderIntent {
            side,
            price,
            quantity: qty,
            reduce_only: false,
        });
    }
    out
}

/// Largest quantity whose worst-case initial margin fits `free` (floor — rounds
/// against over-committing). Unmetered markets skip the cap entirely.
fn cap_by_collateral(qty: u64, price: u64, bps: u16, free: u64) -> u64 {
    if free == UNMETERED_COLLATERAL {
        return qty;
    }
    if qty == 0 || initial_margin(qty, price, bps) as u128 <= free as u128 {
        return qty;
    }
    let (mut lo, mut hi) = (0u64, qty);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if initial_margin(mid, price, bps) as u128 <= free as u128 {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::Persona;
    use solana_sdk::pubkey::Pubkey;

    fn market(num_ticks: u32) -> MarketView {
        MarketView {
            version: 9,
            current_auction_id: 1,
            phase_deadline_slot: 0,
            tick_size: 10,
            orders_per_auction_cap: 128,
            num_ticks,
            oracle: Pubkey::new_unique(),
            phase: 0,
            last_funding_ts: 0,
            oracle_feed_id: [0u8; 32],
            maintenance_margin_bps: 500,
            collateral_mint: Pubkey::new_unique(),
            active_maker_quote_count: 0,
            folded_maker_quote_count: 0,
            window_floor_price: 1000,
            initial_margin_bps: 500,
            max_position_notional: 0,
        }
    }

    fn cfg(persona: Persona) -> TraderConfig {
        TraderConfig {
            persona,
            max_orders: 3,
            base_size: 5,
            aggression_ticks: 2,
            inner_spread_ticks: 1,
            force_side: None,
        }
    }

    #[test]
    fn crossing_buys_price_above_mid_sells_below() {
        let m = market(64); // mid tick 32
        let mut rng = SimRng::new(1);
        let orders = build_orders(
            &m,
            None,
            UNMETERED_COLLATERAL,
            &mut rng,
            &cfg(Persona::Momentum),
        );
        assert!(!orders.is_empty());
        let mid_price = m.tick_size * 32 + m.window_floor_price;
        for o in &orders {
            if o.side == BUY {
                assert!(o.price > mid_price, "buy must cross above mid");
            } else {
                assert!(o.price < mid_price, "sell must cross below mid");
            }
        }
    }

    #[test]
    fn passive_rests_inside_the_spread() {
        let m = market(64);
        let mut rng = SimRng::new(2);
        // inner_spread 1 → passive offset 0 → price == mid (does not cross).
        let orders = build_orders(
            &m,
            None,
            UNMETERED_COLLATERAL,
            &mut rng,
            &cfg(Persona::Passive),
        );
        let mid_price = m.tick_size * 32 + m.window_floor_price;
        for o in &orders {
            assert_eq!(o.price, mid_price, "passive rests at mid (offset 0)");
        }
    }

    #[test]
    fn never_exceeds_max_orders() {
        let m = market(64);
        let mut rng = SimRng::new(3);
        for _ in 0..500 {
            let orders = build_orders(
                &m,
                None,
                UNMETERED_COLLATERAL,
                &mut rng,
                &cfg(Persona::Noise),
            );
            assert!(orders.len() <= 3);
        }
    }

    #[test]
    fn prices_are_tick_aligned_and_in_window() {
        let m = market(64);
        let mut rng = SimRng::new(4);
        let top = m.window_top_price();
        for _ in 0..200 {
            let orders = build_orders(
                &m,
                None,
                UNMETERED_COLLATERAL,
                &mut rng,
                &cfg(Persona::Reckless),
            );
            for o in &orders {
                assert!(o.price >= m.window_floor_price && o.price <= top);
                assert_eq!((o.price - m.window_floor_price) % m.tick_size, 0);
            }
        }
    }

    #[test]
    fn tight_collateral_shrinks_quantity() {
        let m = market(64);
        let mut big = SimRng::new(5);
        let mut small = SimRng::new(5); // same seed → same intents pre-cap
        let unconstrained = build_orders(
            &m,
            None,
            UNMETERED_COLLATERAL,
            &mut big,
            &cfg(Persona::Momentum),
        );
        // Tiny budget: only a handful of base units of margin.
        let constrained = build_orders(&m, None, 5_000, &mut small, &cfg(Persona::Momentum));
        let big_total: u64 = unconstrained.iter().map(|o| o.quantity).sum();
        let small_total: u64 = constrained.iter().map(|o| o.quantity).sum();
        assert!(small_total <= big_total);
    }

    #[test]
    fn zero_collateral_yields_nothing() {
        let m = market(64);
        let mut rng = SimRng::new(6);
        assert!(build_orders(&m, None, 0, &mut rng, &cfg(Persona::Noise)).is_empty());
    }

    #[test]
    fn cap_by_collateral_is_monotone() {
        // More budget never yields fewer units.
        let a = cap_by_collateral(1000, 15_000_000_000, 500, 1_000_000_000);
        let b = cap_by_collateral(1000, 15_000_000_000, 500, 5_000_000_000);
        assert!(b >= a);
        // Unmetered passes through unchanged.
        assert_eq!(cap_by_collateral(777, 100, 500, UNMETERED_COLLATERAL), 777);
    }
}
