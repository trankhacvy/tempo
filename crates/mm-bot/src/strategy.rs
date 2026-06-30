//! The market-maker's quoting strategy: a pure function from (market view, own
//! position, free collateral, config) to a maker-quote ladder. No RPC, no
//! clock, no floats — fully deterministic and unit-testable, the same
//! discipline as `keeper::engine::decide`. The on-chain window is oracle-anchored
//! (`start_auction` re-snaps `window_floor_price` to the oracle band), so
//! centering on the window mid IS centering on the oracle.

use tempo_math::margin::initial_margin;
use tempo_math::tick::tick_to_price;

use tempo_sdk::accounts::{MarketView, PositionView};
use tempo_sdk::ix::Level;

/// Strategy knobs (the subset of `MmConfig` the pure builder needs).
#[derive(Debug, Clone)]
pub struct MmStrategyConfig {
    pub levels: u8,
    pub inner_spread_ticks: u16,
    pub tick_step: u16,
    pub base_size: u64,
    pub size_growth_num: u32,
    pub size_growth_den: u32,
    pub max_inventory: u64,
    pub skew_ticks_max: u16,
    /// Per-round, per-side size variation in basis points (0 = off → a fixed
    /// ladder every round). When set, each rung's size is scaled by a deterministic
    /// pseudo-random factor in `±size_jitter_bps`, seeded by the auction id (and the
    /// side), so the book breathes and leans differently each round without losing
    /// determinism. The reference bot leaves this at 0; the sim turns it up.
    pub size_jitter_bps: u16,
}

/// A two-sided ladder centered on `mid_tick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub mid_tick: u32,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

/// Inventory skew: shift the mid against the current position so the book leans
/// toward flattening. Long (`size > 0`) lowers the mid (asks cheaper / bids
/// lower → biased to sell); short raises it. Integer fixed-point, rounds toward
/// zero, clamped to `±skew_ticks_max`.
fn skew_ticks(size: i64, cfg: &MmStrategyConfig) -> i64 {
    if cfg.max_inventory == 0 || cfg.skew_ticks_max == 0 {
        return 0;
    }
    let max_inv = cfg.max_inventory as i64;
    let raw = size.saturating_mul(cfg.skew_ticks_max as i64) / max_inv;
    raw.clamp(-(cfg.skew_ticks_max as i64), cfg.skew_ticks_max as i64)
}

/// Geometric per-rung size: `base * (num/den)^k`, computed in `u128` and
/// saturated back to `u64` (no floats).
fn rung_size(base: u64, num: u32, den: u32, k: u32) -> u64 {
    let mut acc = base as u128;
    for _ in 0..k {
        acc = acc * num as u128 / den as u128;
    }
    u64::try_from(acc).unwrap_or(u64::MAX)
}

/// Scale `base` by a deterministic factor in `±jitter_bps`, seeded by
/// `(auction_id, k, side)`. A SplitMix64-style mix keeps it pure (same round →
/// same book) while making the size vary round to round and bid≠ask. The factor is
/// floored at 10% so a rung never collapses to nothing.
fn jitter_size(base: u64, jitter_bps: u16, auction_id: u64, k: u32, side: u8) -> u64 {
    if jitter_bps == 0 || base == 0 {
        return base;
    }
    let mut h = auction_id
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((k as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add((side as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    let span = jitter_bps as u64 * 2 + 1;
    let delta = (h % span) as i64 - jitter_bps as i64; // [-jitter_bps, +jitter_bps]
    let factor = (10_000i64 + delta).max(1_000) as u128; // floor at 10%
    u64::try_from(base as u128 * factor / 10_000).unwrap_or(u64::MAX)
}

/// Build the ladder for one Collect window. Returns `None` when the market or
/// collateral leaves nothing safe to quote.
pub fn build_quote(
    market: &MarketView,
    position: Option<&PositionView>,
    free_collateral: u64,
    cfg: &MmStrategyConfig,
) -> Option<Quote> {
    let num_ticks = market.num_ticks;
    if num_ticks == 0 || free_collateral == 0 {
        return None;
    }

    let inventory = position.map(|p| p.size).unwrap_or(0);
    let base_mid = (num_ticks / 2) as i64;
    let mid_tick = (base_mid - skew_ticks(inventory, cfg)).clamp(0, num_ticks as i64 - 1) as u32;

    // Room left on the inventory-increasing side before |size| would breach the
    // cap; a full fill on that side must not push the position past it.
    let abs_inv = inventory.unsigned_abs();
    let room = cfg.max_inventory.saturating_sub(abs_inv);

    let mut bids: Vec<Level> = Vec::new();
    let mut asks: Vec<Level> = Vec::new();
    let mut bid_cum: u64 = 0;
    let mut ask_cum: u64 = 0;

    for k in 0..cfg.levels as u32 {
        let offset = cfg.inner_spread_ticks as u32 + k * cfg.tick_step as u32;
        let size = rung_size(cfg.base_size, cfg.size_growth_num, cfg.size_growth_den, k);
        if size == 0 {
            continue;
        }

        // Bid rung: tick = mid - offset, valid only while it stays in-window.
        if offset <= mid_tick {
            let raw = jitter_size(size, cfg.size_jitter_bps, market.current_auction_id, k, 0);
            // When long, buying increases inventory → cap the bid side to `room`.
            let capped = if inventory > 0 {
                raw.min(room.saturating_sub(bid_cum))
            } else {
                raw
            };
            if capped > 0 {
                bid_cum = bid_cum.saturating_add(capped);
                bids.push(Level {
                    offset: offset as u16,
                    size: capped,
                });
            }
        }

        // Ask rung: tick = mid + offset, valid only while it stays in-window.
        if mid_tick + offset < num_ticks {
            let raw = jitter_size(size, cfg.size_jitter_bps, market.current_auction_id, k, 1);
            // When short, selling increases inventory → cap the ask side to `room`.
            let capped = if inventory < 0 {
                raw.min(room.saturating_sub(ask_cum))
            } else {
                raw
            };
            if capped > 0 {
                ask_cum = ask_cum.saturating_add(capped);
                asks.push(Level {
                    offset: offset as u16,
                    size: capped,
                });
            }
        }
    }

    if bids.is_empty() && asks.is_empty() {
        return None;
    }

    scale_to_collateral(market, mid_tick, &mut bids, &mut asks, free_collateral);
    bids.retain(|l| l.size > 0);
    asks.retain(|l| l.size > 0);
    if bids.is_empty() && asks.is_empty() {
        return None;
    }

    Some(Quote {
        mid_tick,
        bids,
        asks,
    })
}

/// Scale every rung down uniformly so the worst-case initial margin of the whole
/// ladder fits inside `free_collateral`. Uses the same `initial_margin` the
/// program reserves, so the MM can never post a ladder it cannot back.
fn scale_to_collateral(
    market: &MarketView,
    mid_tick: u32,
    bids: &mut [Level],
    asks: &mut [Level],
    free_collateral: u64,
) {
    let bps = market.effective_initial_margin_bps();
    let window_top = market.window_top_price();

    let bid_price = |offset: u16| -> u64 {
        tick_to_price(
            mid_tick.saturating_sub(offset as u32),
            market.window_floor_price,
            market.tick_size,
            market.num_ticks,
        )
        .unwrap_or(window_top)
    };

    let mut total: u128 = 0;
    for l in bids.iter() {
        total += initial_margin(l.size, bid_price(l.offset), bps) as u128;
    }
    for l in asks.iter() {
        // A sell can clear no higher than the window top — the worst case.
        total += initial_margin(l.size, window_top, bps) as u128;
    }

    if total == 0 || total <= free_collateral as u128 {
        return;
    }
    // size' = size * free / total (floor, rounds against over-committing).
    let scale = |size: u64| -> u64 { ((size as u128 * free_collateral as u128) / total) as u64 };
    for l in bids.iter_mut() {
        l.size = scale(l.size);
    }
    for l in asks.iter_mut() {
        l.size = scale(l.size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;

    fn market(num_ticks: u32) -> MarketView {
        MarketView {
            version: 9,
            current_auction_id: 1,
            phase_deadline_slot: 0,
            tick_size: 10,
            orders_per_auction_cap: 64,
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

    fn position(size: i64) -> PositionView {
        PositionView {
            owner: Pubkey::new_unique(),
            market: Pubkey::new_unique(),
            size,
            entry_price: 1000,
            collateral: 0,
            realized_pnl: 0,
            margin_mode: 0,
        }
    }

    fn cfg() -> MmStrategyConfig {
        MmStrategyConfig {
            levels: 3,
            inner_spread_ticks: 1,
            tick_step: 1,
            base_size: 100,
            size_growth_num: 1,
            size_growth_den: 1,
            max_inventory: 10_000,
            skew_ticks_max: 2,
            size_jitter_bps: 0,
        }
    }

    #[test]
    fn symmetric_at_zero_inventory() {
        let q = build_quote(&market(64), Some(&position(0)), u64::MAX / 2, &cfg()).unwrap();
        assert_eq!(q.mid_tick, 32);
        assert_eq!(q.bids.len(), 3);
        assert_eq!(q.asks.len(), 3);
        // mirror offsets and equal sizes.
        for (b, a) in q.bids.iter().zip(q.asks.iter()) {
            assert_eq!(b.offset, a.offset);
            assert_eq!(b.size, a.size);
        }
        assert_eq!(q.bids[0].offset, 1);
        assert_eq!(q.bids[2].offset, 3);
    }

    #[test]
    fn long_inventory_skews_mid_down_and_caps_bids() {
        // Long 9900 of 10000 cap: mid shifts down and bid room is only 100, so a
        // full bid fill cannot breach the cap. Asks (the flattening side) are free.
        let q = build_quote(&market(64), Some(&position(9900)), u64::MAX / 2, &cfg()).unwrap();
        assert!(q.mid_tick < 32, "long inventory lowers the mid");
        let bid_total: u64 = q.bids.iter().map(|l| l.size).sum();
        assert!(
            bid_total <= 100,
            "bids capped to remaining room: {bid_total}"
        );
        let ask_total: u64 = q.asks.iter().map(|l| l.size).sum();
        assert!(ask_total > bid_total);
    }

    #[test]
    fn short_inventory_mirrors() {
        let q = build_quote(&market(64), Some(&position(-9900)), u64::MAX / 2, &cfg()).unwrap();
        assert!(q.mid_tick > 32, "short inventory raises the mid");
        let ask_total: u64 = q.asks.iter().map(|l| l.size).sum();
        assert!(
            ask_total <= 100,
            "asks capped to remaining room: {ask_total}"
        );
        let bid_total: u64 = q.bids.iter().map(|l| l.size).sum();
        assert!(bid_total > ask_total);
    }

    #[test]
    fn rungs_clamped_to_window_edges() {
        // num_ticks=4 → mid=2; inner_spread=1, step=1, levels=3.
        // bids: offset 1 (tick1 ok), 2 (tick0 ok), 3 (tick -1 invalid → dropped).
        // asks: offset 1 (tick3 ok), 2 (tick4 invalid), 3 invalid → only 1 ask.
        let q = build_quote(&market(4), Some(&position(0)), u64::MAX / 2, &cfg()).unwrap();
        assert_eq!(q.mid_tick, 2);
        assert_eq!(q.bids.len(), 2);
        assert_eq!(q.asks.len(), 1);
        assert!(q.bids.iter().all(|l| (l.offset as u32) <= q.mid_tick));
        assert!(q.asks.iter().all(|l| q.mid_tick + (l.offset as u32) < 4));
    }

    #[test]
    fn collateral_cap_scales_sizes_down() {
        // Tiny free collateral forces the ladder down vs an unconstrained build.
        let big = build_quote(&market(64), Some(&position(0)), u64::MAX / 2, &cfg()).unwrap();
        let small = build_quote(&market(64), Some(&position(0)), 5_000, &cfg()).unwrap();
        let big_total: u64 = big.bids.iter().chain(&big.asks).map(|l| l.size).sum();
        let small_total: u64 = small.bids.iter().chain(&small.asks).map(|l| l.size).sum();
        assert!(small_total < big_total);
    }

    #[test]
    fn zero_free_collateral_yields_no_quote() {
        assert!(build_quote(&market(64), Some(&position(0)), 0, &cfg()).is_none());
    }

    #[test]
    fn single_level_reproduces_legacy_book() {
        let mut c = cfg();
        c.levels = 1;
        let q = build_quote(&market(64), None, u64::MAX / 2, &c).unwrap();
        assert_eq!(q.bids.len(), 1);
        assert_eq!(q.asks.len(), 1);
        assert_eq!(q.bids[0].offset, 1);
        assert_eq!(q.asks[0].offset, 1);
    }
}
