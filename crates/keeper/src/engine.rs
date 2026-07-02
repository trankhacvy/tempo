use solana_sdk::pubkey::Pubkey;

use tempo_sdk::accounts::SlabOrder;

use crate::snapshot::MarketSnapshot;

pub const PHASE_COLLECT: u8 = 0;
pub const PHASE_ACCUMULATING: u8 = 1;
pub const PHASE_DISCOVERED: u8 = 2;
pub const PHASE_SETTLING: u8 = 3;

/// The next action the keeper should drive, derived purely from a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Collect window still open — wait.
    Idle,
    /// Fold resting orders + unfolded maker quotes into the histogram.
    Accumulate {
        chunks: Vec<(u32, u32)>,
        quotes: Vec<Pubkey>,
    },
    /// Completeness satisfied — publish the cross.
    Discover,
    /// Pull fills: accumulated slab orders + folded-not-settled maker quotes.
    Settle {
        orders: Vec<SlabOrder>,
        quotes: Vec<Pubkey>,
    },
    /// Slab empty and quotes settled — roll to the next round.
    Roll { oracle: Pubkey },
}

/// The pure phase state machine. Optimistic by design: it emits the action it
/// believes is next; the program's phase guards + idempotency make a wrong guess a
/// benign no-op (so a crash or a second replica is always safe).
pub fn decide(s: &MarketSnapshot, now_slot: u64, chunk_size: u32) -> Plan {
    match s.market.phase {
        PHASE_COLLECT => {
            if now_slot < s.market.phase_deadline_slot {
                Plan::Idle
            } else {
                // Deadline reached: emit the chunk range to fold (and, for an empty
                // round, to force the Collect → Accumulating transition).
                accumulate_plan(s, chunk_size)
            }
        }
        PHASE_ACCUMULATING => {
            // PERF-1 (known-issues §2.1): the redundant market order-count mirrors are
            // gone; order completeness is the authoritative slab scan `all_resting_folded`
            // (the same signal the on-chain `all_active_orders_accumulated` finalize gate
            // uses). Maker-quote completeness still rides the market quote counters.
            let makers_done =
                s.market.folded_maker_quote_count == s.market.active_maker_quote_count;
            if makers_done && s.all_resting_folded() {
                Plan::Discover
            } else {
                accumulate_plan(s, chunk_size)
            }
        }
        PHASE_DISCOVERED | PHASE_SETTLING => {
            let orders = s.accumulated_orders();
            let quotes = s.unsettled_folded_quotes();
            if orders.is_empty() && quotes.is_empty() {
                // Covers the empty-round roll from Discovered (no settle ever ran).
                Plan::Roll {
                    oracle: s.market.oracle,
                }
            } else {
                Plan::Settle { orders, quotes }
            }
        }
        _ => Plan::Idle,
    }
}

/// Build the chunk ranges over the whole slab capacity, plus the not-yet-folded
/// quotes. The full range is re-emitted each tick (`process_chunk` skips already-
/// folded/empty slots, so it is idempotent); this keeps the logic correct without
/// per-chunk bookkeeping and makes a dropped chunk simply retry next tick.
fn accumulate_plan(s: &MarketSnapshot, chunk_size: u32) -> Plan {
    let cap = s.market.orders_per_auction_cap;
    let step = chunk_size.max(1);
    let mut chunks = Vec::new();
    let mut start = 0u32;
    while start < cap {
        let count = step.min(cap - start);
        chunks.push((start, count));
        start += count;
    }
    Plan::Accumulate {
        chunks,
        quotes: s.unfolded_quotes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempo_sdk::accounts::{MakerQuoteView, MarketView, SlabOrder};

    fn market(phase: u8) -> MarketView {
        MarketView {
            version: 9,
            current_auction_id: 5,
            phase_deadline_slot: 100,
            tick_size: 10,
            orders_per_auction_cap: 4,
            num_ticks: 64,
            oracle: Pubkey::new_from_array([9u8; 32]),
            phase,
            last_funding_ts: 0,
            oracle_feed_id: [0u8; 32],
            maintenance_margin_bps: 0,
            collateral_mint: Pubkey::default(),
            active_maker_quote_count: 0,
            folded_maker_quote_count: 0,
            window_floor_price: 10,
            initial_margin_bps: 0,
            max_position_notional: 0,
            num_slab_shards: 1,
        }
    }

    fn order(slot: u32, order_id: u64, status: u8) -> SlabOrder {
        SlabOrder {
            slot,
            order_id,
            trader: Pubkey::new_from_array([1u8; 32]),
            side: 0,
            status,
            price: 100,
            quantity: 5,
            remaining: 5,
            expires_at_auction: 0,
            arm_auction_id: 0,
            shard_id: 0,
        }
    }

    fn snap(market: MarketView, slab: Vec<SlabOrder>) -> MarketSnapshot {
        MarketSnapshot {
            market,
            slab,
            clearing: None,
            quotes: Vec::new(),
        }
    }

    fn quote(folded: u64, settled: u64, status: u8) -> (Pubkey, MakerQuoteView) {
        (
            Pubkey::new_unique(),
            MakerQuoteView {
                maker: Pubkey::new_from_array([2u8; 32]),
                market: Pubkey::new_from_array([3u8; 32]),
                sequence: 0,
                mid_tick: 0,
                status,
                num_bids: 0,
                num_asks: 0,
                folded_auction_id: folded,
                settled_auction_id: settled,
            },
        )
    }

    #[test]
    fn collect_before_deadline_is_idle() {
        let s = snap(market(PHASE_COLLECT), vec![order(0, 1, 1)]);
        assert_eq!(decide(&s, 99, 256), Plan::Idle);
    }

    #[test]
    fn collect_after_deadline_accumulates() {
        let s = snap(market(PHASE_COLLECT), vec![order(0, 1, 1)]);
        match decide(&s, 100, 256) {
            Plan::Accumulate { chunks, .. } => assert_eq!(chunks, vec![(0, 4)]),
            other => panic!("expected Accumulate, got {other:?}"),
        }
    }

    #[test]
    fn empty_round_after_deadline_still_accumulates_to_transition() {
        // No orders at all: must still emit a chunk to force Collect -> Accumulating.
        let s = snap(market(PHASE_COLLECT), vec![]);
        match decide(&s, 100, 256) {
            Plan::Accumulate { chunks, quotes } => {
                assert_eq!(chunks, vec![(0, 4)]);
                assert!(quotes.is_empty());
            }
            other => panic!("expected Accumulate, got {other:?}"),
        }
    }

    #[test]
    fn accumulating_resting_order_remains_keeps_accumulating() {
        // A slot is still Resting (status 1) — the authoritative slab scan keeps
        // folding (PERF-1: no counters; this IS the completeness signal).
        let m = market(PHASE_ACCUMULATING);
        let s = snap(m, vec![order(0, 1, 2), order(1, 2, 1)]);
        assert!(matches!(decide(&s, 200, 256), Plan::Accumulate { .. }));
    }

    #[test]
    fn accumulating_all_folded_discovers() {
        // Every slab slot is folded (status 2) — order completeness satisfied, so
        // Discover (matches the on-chain `all_active_orders_accumulated` gate).
        let m = market(PHASE_ACCUMULATING);
        let s = snap(m, vec![order(0, 1, 2), order(1, 2, 2)]);
        assert_eq!(decide(&s, 200, 256), Plan::Discover);
    }

    #[test]
    fn accumulating_empty_slab_discovers() {
        // No resting orders and no maker quotes — completeness trivially holds.
        let m = market(PHASE_ACCUMULATING);
        let s = snap(m, vec![]);
        assert_eq!(decide(&s, 200, 256), Plan::Discover);
    }

    #[test]
    fn accumulating_waits_on_unfolded_maker_quote() {
        let mut m = market(PHASE_ACCUMULATING);
        m.active_maker_quote_count = 1;
        m.folded_maker_quote_count = 0;
        let mut s = snap(m, vec![]);
        s.quotes = vec![quote(u64::MAX, u64::MAX, 1)]; // never folded
        match decide(&s, 200, 256) {
            Plan::Accumulate { quotes, .. } => assert_eq!(quotes.len(), 1),
            other => panic!("expected Accumulate, got {other:?}"),
        }
    }

    #[test]
    fn discovered_with_orders_settles() {
        let s = snap(market(PHASE_DISCOVERED), vec![order(0, 1, 2)]);
        match decide(&s, 200, 256) {
            Plan::Settle { orders, .. } => assert_eq!(orders.len(), 1),
            other => panic!("expected Settle, got {other:?}"),
        }
    }

    #[test]
    fn discovered_empty_book_rolls() {
        let s = snap(market(PHASE_DISCOVERED), vec![]);
        match decide(&s, 200, 256) {
            Plan::Roll { oracle } => assert_eq!(oracle, Pubkey::new_from_array([9u8; 32])),
            other => panic!("expected Roll, got {other:?}"),
        }
    }

    #[test]
    fn settling_with_folded_unsettled_quote_settles_not_rolls() {
        // Slab is fully consumed, but a maker quote folded this round (5) is
        // unsettled (settled 4) — must settle, never roll.
        let mut s = snap(market(PHASE_SETTLING), vec![order(0, 1, 3)]);
        s.quotes = vec![quote(5, 4, 1)];
        match decide(&s, 200, 256) {
            Plan::Settle { orders, quotes } => {
                assert!(orders.is_empty()); // consumed, not accumulated
                assert_eq!(quotes.len(), 1);
            }
            other => panic!("expected Settle, got {other:?}"),
        }
    }

    #[test]
    fn settling_all_done_rolls() {
        let mut s = snap(market(PHASE_SETTLING), vec![order(0, 1, 3)]);
        s.quotes = vec![quote(5, 5, 1)]; // folded and settled this round
        assert!(matches!(decide(&s, 200, 256), Plan::Roll { .. }));
    }

    #[test]
    fn chunking_respects_chunk_size() {
        let mut m = market(PHASE_COLLECT);
        m.orders_per_auction_cap = 10;
        let s = snap(m, vec![order(0, 1, 1)]);
        match decide(&s, 100, 4) {
            Plan::Accumulate { chunks, .. } => {
                assert_eq!(chunks, vec![(0, 4), (4, 4), (8, 2)]);
            }
            other => panic!("expected Accumulate, got {other:?}"),
        }
    }
}
