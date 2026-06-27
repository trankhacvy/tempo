use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    state::{AuctionHistogramHeader, AuctionPhase, Market, OrderSlabHeader, COLLECT_WINDOW_SLOTS},
    traits::{AccountDeserialize, PdaSeeds},
};

/// Reset a market's round state and reopen `Collect`: zero the order slab and
/// histogram, bump both to `next_id`, reset the market counters, and open a fresh
/// collection window at `slot + COLLECT_WINDOW_SLOTS`. Validates that the slab and
/// histogram belong to this market. Shared by `start_auction` (after its
/// settled-slab preconditions) and `force_reset` (after its authority check).
pub fn reset_round_to_collect(
    program_id: &Address,
    market: &AccountView,
    histogram: &AccountView,
    order_slab: &AccountView,
    num_ticks: u32,
    next_id: u64,
    slot: u64,
) -> ProgramResult {
    let market_key = *market.address();

    // --- order slab: validate, zero every slot, bump the round ---
    {
        let mut slab_account = *order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;

        {
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(order_slab, program_id, slab.bump)?;
        }

        // Zero every slot (sets each Order's status byte to Empty == 0) so the
        // next round reuses them; Consumed slots are otherwise never freed.
        let slots_off = OrderSlabHeader::slots_offset();
        slab_data[slots_off..].iter_mut().for_each(|b| *b = 0);

        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        header.set_auction_id(next_id);
        header.set_next_order_id(0);
        header.set_count(0);
        // Reset the forward allocation cursor for the fresh (empty) slab so the
        // next round starts allocating at slot 0 (known-issues §2.7).
        header.set_next_free_hint(0);
    }

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
        market.set_accumulated_order_count(0);
        market.set_active_order_count(0);
        // Per-round maker-quote fold counter resets; the active count persists
        // (quotes survive across rounds, unlike the ephemeral slab).
        market.set_folded_maker_quote_count(0);
        market.set_phase_deadline_slot(slot.saturating_add(COLLECT_WINDOW_SLOTS));
        market.phase = AuctionPhase::Collect as u8;
    }

    Ok(())
}
