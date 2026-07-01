use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::ResetShard,
    state::{
        all_accumulated_orders_settled, count_resting_orders, read_order, write_order,
        AuctionPhase, Market, Order, OrderSlabHeader, OrderStatus,
    },
    traits::{AccountDeserialize, PdaSeeds},
};

/// Processes the ResetShard instruction — Stage A sharding, permissionless.
///
/// Compacts one settled shard and re-arms it for the next round, then increments
/// `Market.shards_ready`. `start_auction` rolls the round only once every shard is ready
/// (`shards_ready == num_slab_shards`), so this is the sharded equivalent of the old
/// single-slab zeroing in `reset_round_to_collect` — spread across one tx per shard
/// (parallel, and scalable to `MAX_SLAB_SHARDS`).
///
/// Stage B resting orders: rather than zero every slot, it frees only `Consumed`/`Empty`
/// slots and KEEPS each `Resting` survivor (its leftover carries to the next round), then
/// recomputes `count`/`resting_count` from the compacted shard.
///
/// Exactly-once per shard per round: the shard is required to belong to the current
/// round (`shard.auction_id == market.current_auction_id`) and is bumped to the next id
/// here, so a repeat call fails the id check. Runs only post-clearing (Settling /
/// Discovered) and only on a fully-SETTLED shard (no order still `Accumulated`), so it
/// cannot drop a folded-but-unsettled order (the freeze model — clearing-protocol §4).
pub fn process_reset_shard(
    program_id: &Address,
    accounts: &[AccountView],
    _instruction_data: &[u8],
) -> ProgramResult {
    let ix = ResetShard::try_from((_instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- read market: phase + round + shard count ---
    let (auction_id, num_slab_shards) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        let phase = market.phase()?;
        // Only reset after clearing, so an empty shard can't be reset mid-Collect and let
        // the round roll while other shards still hold live orders.
        if phase != AuctionPhase::Settling && phase != AuctionPhase::Discovered {
            return Err(TempoProgramError::AuctionWrongPhase.into());
        }
        (market.current_auction_id(), market.num_slab_shards())
    };

    let next_id = auction_id
        .checked_add(1)
        .ok_or(TempoProgramError::MathOverflow)?;

    // --- validate the shard + require it drained + belongs to the current round ---
    {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;

        let capacity = {
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
            if slab.shard_id() >= num_slab_shards {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            // Exactly-once guard: only a shard still tagged for the current round can be
            // reset; resetting bumps it to next_id, so a second call fails here.
            if slab.auction_id() != auction_id {
                return Err(TempoProgramError::AuctionIdMismatch.into());
            }
            slab.capacity()
        };

        // Freeze model (Stage B resting orders): the shard must be fully SETTLED before it
        // can roll — i.e. no order is still `Accumulated` (folded but not yet settled). We
        // can no longer gate on `count == 0`: with resting orders the survivors stay in the
        // book (`count > 0` across the roll), so draining-to-empty would wedge the round
        // forever. This is an authoritative per-shard scan (Design Z / DDR-1) — the settle
        // equivalent of finalize's `all_active_orders_accumulated` fold gate.
        if !all_accumulated_orders_settled(&slab_data, capacity)? {
            return Err(TempoProgramError::AuctionNotComplete.into());
        }

        // Compact the shard for the next round: free every `Consumed`/`Empty` slot (Consumed
        // slots are otherwise never reclaimed) but KEEP each `Resting` survivor in place —
        // its reduced `remaining`, `worst_price`, `reserved_margin`, `order_id` and expiry
        // carry forward, and next round's `process_chunk` re-folds it (Resting → Accumulated).
        for i in 0..capacity {
            let o = read_order(&slab_data, capacity, i)?;
            if o.status()? != OrderStatus::Resting {
                write_order(&mut slab_data, capacity, i, &Order::empty())?;
            }
        }

        // Recompute the counters authoritatively from the compacted shard (no trust in a
        // running counter across the settle→roll boundary): both `count` (live orders) and
        // `resting_count` (unfolded-this-round hint) equal the surviving Resting orders.
        let survivors = count_resting_orders(&slab_data, capacity)?;

        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        header.set_auction_id(next_id);
        // `next_order_id` is intentionally NOT reset: it stays monotonic so a new order can
        // never reuse a surviving order's id (which would make find-by-id ambiguous).
        header.set_count(survivors);
        header.set_next_free_hint(0);
        header.set_resting_count(survivors);
        header.set_folded_auction_id(u64::MAX);
    }

    // --- market: this shard is ready for the next round ---
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_shards_ready(
            market
                .shards_ready()
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
    }

    Ok(())
}
