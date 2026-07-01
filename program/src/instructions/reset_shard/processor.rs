use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::ResetShard,
    state::{AuctionPhase, Market, OrderSlabHeader},
    traits::{AccountDeserialize, PdaSeeds},
};

/// Processes the ResetShard instruction — Stage A sharding, permissionless.
///
/// Zeroes one drained shard's slots and re-arms it for the next round, then increments
/// `Market.shards_ready`. `start_auction` rolls the round only once every shard is ready
/// (`shards_ready == num_slab_shards`), so this is the sharded equivalent of the old
/// single-slab zeroing in `reset_round_to_collect` — spread across one tx per shard
/// (parallel, and scalable to `MAX_SLAB_SHARDS`).
///
/// Exactly-once per shard per round: the shard is required to belong to the current
/// round (`shard.auction_id == market.current_auction_id`) and is bumped to the next id
/// here, so a repeat call fails the id check. Runs only post-clearing (Settling /
/// Discovered) and only on a fully-settled shard (`count == 0`), so it cannot drop live
/// orders (the freeze model — clearing-protocol §4).
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

        {
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
            // Freeze model: the shard must be fully settled before it can roll.
            if slab.count() != 0 {
                return Err(TempoProgramError::AuctionNotComplete.into());
            }
        }

        // Zero every slot (Consumed slots are otherwise never freed for reuse), then
        // re-arm the header for the next round.
        let slots_off = OrderSlabHeader::slots_offset();
        slab_data[slots_off..].iter_mut().for_each(|b| *b = 0);

        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        header.set_auction_id(next_id);
        header.set_next_order_id(0);
        header.set_count(0);
        header.set_next_free_hint(0);
        header.set_resting_count(0);
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
