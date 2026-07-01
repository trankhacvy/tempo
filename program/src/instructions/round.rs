use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    state::{AuctionHistogramHeader, AuctionPhase, Market, COLLECT_WINDOW_SLOTS},
    traits::{AccountDeserialize, PdaSeeds},
};

/// Reset a market's round state and reopen `Collect`: zero the histogram, bump it to
/// `next_id`, reset the maker-quote fold counter, reset the shard aggregates
/// (`shards_pending = 0` — only shards with unfolded orders get counted, `shards_ready = 0`), and open a fresh
/// collection window at `slot + COLLECT_WINDOW_SLOTS`. Validates the histogram belongs
/// to this market. Shared by `start_auction` (after `shards_ready == num_slab_shards`)
/// and `force_reset` (after its authority check).
///
/// Stage A sharding: the OrderSlab shards are NOT touched here — they are drained and
/// zeroed one-per-tx by `reset_shard` before the roll (a market may have too many shards
/// for one tx). `force_reset` therefore leaves shards dirty; an admin must `reset_shard`
/// each afterward (it is an escape hatch, not a hot path).
pub fn reset_round_to_collect(
    program_id: &Address,
    market: &AccountView,
    histogram: &AccountView,
    num_ticks: u32,
    next_id: u64,
    slot: u64,
) -> ProgramResult {
    let market_key = *market.address();

    // --- histogram: validate, zero all buckets, bump the round ---
    {
        let mut hist_account = *histogram;
        let mut hist_data = hist_account.try_borrow_mut()?;

        {
            let hist = AuctionHistogramHeader::from_bytes(&hist_data)?;
            if hist.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            hist.validate_pda(histogram, program_id, hist.bump)?;
            if hist.num_ticks() != num_ticks {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
        }

        let buckets_off = AuctionHistogramHeader::buckets_offset();
        hist_data[buckets_off..].iter_mut().for_each(|b| *b = 0);

        let header = AuctionHistogramHeader::from_bytes_mut(&mut hist_data)?;
        header.set_auction_id(next_id);
        header.set_accumulated_count(0);
    }

    // --- market: reset counters, open a fresh Collect window ---
    {
        let mut market_account = *market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_current_auction_id(next_id);
        // Per-round maker-quote fold counter resets; the active count persists
        // (quotes survive across rounds, unlike the ephemeral slab).
        market.set_folded_maker_quote_count(0);
        // Stage A: reset the shard aggregates for the new round. `shards_pending` counts
        // only shards that hold unfolded orders, so it starts at 0 and is bumped by the
        // first submit into each shard (empty shards stay uncounted). `shards_ready` (the
        // drain/roll gate) resets to 0 — every shard must be reset again before the next roll.
        market.set_shards_pending(0);
        market.set_shards_ready(0);
        market.set_phase_deadline_slot(slot.saturating_add(COLLECT_WINDOW_SLOTS));
        market.phase = AuctionPhase::Collect as u8;
    }

    Ok(())
}
