use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    events::ChunkProcessedEvent,
    instructions::ProcessChunk,
    state::{
        all_active_orders_accumulated, fold, read_order, read_region, write_order,
        AuctionHistogramHeader, AuctionPhase, Market, OrderSide, OrderSlabHeader, OrderStatus,
        Region,
    },
    traits::{AccountDeserialize, EventSerialize, PdaSeeds},
    utils::emit_event,
};

/// Processes the ProcessChunk instruction — Phase 1 ACCUMULATE
/// (clearing-protocol §3). Permissionless. Folds a bounded slice of resting
/// orders into the histogram, marks them accumulated, and bumps the
/// histogram's authoritative accumulated-order counter (PERF-1: the market no
/// longer mirrors it; it still write-locks the market only for the one-time
/// Collect→Accumulating phase flip).
///
/// Commutativity (clearing-protocol §4.1): folding is integer addition into a
/// bucket, so the final histogram is identical regardless of which cranker
/// processes which chunk in which order. The per-order `Accumulated` flag
/// prevents a double-fold; the completeness check in `finalize_clear` prevents
/// a skip.
pub fn process_process_chunk(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProcessChunk::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- read market params + maybe transition Collect -> Accumulating ---
    let (tick_size, num_ticks, window_floor, auction_id) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        let phase = market.phase()?;
        if phase != AuctionPhase::Collect && phase != AuctionPhase::Accumulating {
            return Err(TempoProgramError::AuctionWrongPhase.into());
        }
        (
            market.tick_size(),
            market.num_ticks(),
            market.window_floor_price(),
            market.current_auction_id(),
        )
    };

    // Transition phase on the first chunk.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        if market.phase == AuctionPhase::Collect as u8 {
            // Hold the book open until the collection window closes, so orders
            // submitted anywhere in the window join the same batch.
            if Clock::get()?.slot < market.phase_deadline_slot() {
                return Err(TempoProgramError::AuctionWindowOpen.into());
            }
            market.phase = AuctionPhase::Accumulating as u8;
        }
    }

    // --- validate histogram + slab belong to this market and auction ---
    {
        let hist_data = ix.accounts.histogram.try_borrow()?;
        let hist = AuctionHistogramHeader::from_bytes(&hist_data)?;
        if hist.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        hist.validate_pda(ix.accounts.histogram, program_id, hist.bump)?;
        if hist.auction_id() != auction_id {
            return Err(TempoProgramError::AuctionIdMismatch.into());
        }
        if hist.num_ticks() != num_ticks {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
    }

    // --- fold a bounded slice of resting orders ---
    let mut folded: u64 = 0;
    let (accumulated_total, shard_newly_complete) = {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;
        let mut hist_account = *ix.accounts.histogram;
        let mut hist_data = hist_account.try_borrow_mut()?;

        let capacity = {
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
            slab.capacity()
        };

        let start = ix.data.start_index;
        if start >= capacity {
            return Err(TempoProgramError::OrderNotFound.into());
        }
        let end = start.saturating_add(ix.data.max_count).min(capacity);

        // We need the price->tick mapping, which lives on Market. Recompute it
        // here cheaply from tick_size/num_ticks without re-borrowing Market:
        // tick = price / tick_size - 1, validated to be in [0, num_ticks).
        for i in start..end {
            let order = read_order(&slab_data, capacity, i)?;
            if order.status != OrderStatus::Resting as u8 {
                continue; // empty, already accumulated, or consumed
            }

            let tick =
                crate::state::price_to_tick_raw(order.price, window_floor, tick_size, num_ticks)?;

            // Slab orders are taker-only (§1.3), so they fold only into the two
            // taker regions of the dual auction (system-design §1): a taker sell is
            // the supply side of the bid auction; a taker buy is the demand side of
            // the ask auction. The maker regions (`BidDemand`/`AskSupply`) are fed
            // exclusively by `process_maker_quote` from the `MakerQuote` book.
            let region = match OrderSide::from_u8(order.side)? {
                OrderSide::Sell => Region::BidSupply,
                OrderSide::Buy => Region::AskDemand,
            };
            // Fold-time `cum_before` snapshot (known-issues §2.7, mirrors the §1.6
            // MakerQuote fix): the region/tick bucket value *immediately before*
            // this order folds is exactly its prefix among same-bucket orders, in
            // fold order. `settle_fill` reads it for O(1) marginal-tick rationing
            // instead of re-scanning the slab. The telescoping prefixes tile
            // `[0, total_qty)` for ANY crank fold order, so Σ fills == vol_alloc
            // (aggregate OI is fold-order-independent; only ≤1-lot floor dust is
            // not — the same guarantee the maker path relies on).
            let cum_before = read_region(&hist_data, num_ticks, region, tick)?;
            fold(&mut hist_data, num_ticks, region, tick, order.remaining)?;

            // mark accumulated (exactly once), recording the prefix snapshot
            let mut updated = order;
            updated.status = OrderStatus::Accumulated as u8;
            updated.cum_before = cum_before;
            write_order(&mut slab_data, capacity, i, &updated)?;

            folded = folded
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?;
        }

        // bump histogram accumulated_count (the authoritative folded count, PERF-1).
        let hist = AuctionHistogramHeader::from_bytes_mut(&mut hist_data)?;
        let c = hist
            .accumulated_count()
            .checked_add(folded)
            .ok_or(TempoProgramError::MathOverflow)?;
        hist.set_accumulated_count(c);

        // Stage A completeness: decrement this shard's resting counter, then decide
        // whether the shard just left the market's completeness set. `resting_count` is the
        // per-shard authoritative-by-locality counter (maintained in the same borrow as the
        // order status above). A shard was ADDED to `shards_pending` by the first submit into
        // it (resting_count 0→1); it must leave EXACTLY when its last unfolded order is folded.
        //
        // The gate is `folded > 0 && rc == 0`:
        //  - `folded > 0` ⟹ this call actually folded ≥1 order, so an EMPTY shard (never
        //    submitted to; never counted) is a no-op and never decrements — no per-shard crank
        //    obligation, and `shards_pending` stays balanced against the submit increments.
        //  - a re-crank after completion folds 0 ⟹ `folded > 0` is false ⟹ fires exactly once.
        // The `folded_auction_id` guard is belt-and-suspenders (redundant given `folded > 0`),
        // and the authoritative slab scan confirms the counter so a corrupt `resting_count`
        // can never let finalize proceed with an unfolded order (the censorship guarantee,
        // amortized per-shard at fold time).
        let slab = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        // `folded` counts orders folded this call (≤ capacity ≤ u32::MAX), safe to narrow.
        let rc = slab.resting_count().saturating_sub(folded as u32);
        slab.set_resting_count(rc);
        let prev_folded_id = slab.folded_auction_id();
        let newly_complete = folded > 0
            && rc == 0
            && prev_folded_id != auction_id
            && all_active_orders_accumulated(&slab_data, capacity)?;
        if newly_complete {
            OrderSlabHeader::from_bytes_mut(&mut slab_data)?.set_folded_auction_id(auction_id);
        }
        (c, newly_complete)
    };

    // A shard that just became fully folded decrements the market's completeness
    // aggregate exactly once. `finalize_clear` gates on `shards_pending == 0` (O(1)),
    // so the whole book's completeness is a single check instead of a full slab scan.
    if shard_newly_complete {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_shards_pending(market.shards_pending().saturating_sub(1));
    }

    // Emit event via CPI (carries `folded`/`accumulated_total`; no log! — this is
    // the hot crank path and logging is the costliest avoidable op, §1).
    let event = ChunkProcessedEvent {
        market: market_key,
        cranker: *ix.accounts.cranker.address(),
        auction_id,
        folded,
        accumulated_total,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
