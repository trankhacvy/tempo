use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::{reset_round_to_collect, ForceReset},
    state::{Market, OrderSlabHeader},
    traits::{AccountDeserialize, PdaSeeds},
};

/// Processes the ForceReset instruction — an authority-gated escape hatch that
/// abandons a wedged round and reopens `Collect`, regardless of phase or
/// unsettled orders (system-design §7). It bumps the auction id, zeroes the
/// histogram, resets the counters + shard aggregates, and opens a fresh collection
/// window. This is an operational backstop for a stuck round, NOT a normal path — the
/// permissionless cranks drain a round under the freeze model on their own.
///
/// Stage A sharding: this zeroes the ONE passed slab shard (the escape hatch can clear a
/// wedged shard that `reset_shard` cannot, since `reset_shard` needs a drained,
/// post-clearing shard). It re-arms `shards_pending`/`shards_ready` for the new round. A
/// market with more than one shard must `force_reset` each dirty shard.
pub fn process_force_reset(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ForceReset::try_from((instruction_data, accounts))?;

    // --- validate authority + capture params ---
    let (num_ticks, auction_id) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;
        (market.num_ticks(), market.current_auction_id())
    };

    let next_id = auction_id
        .checked_add(1)
        .ok_or(TempoProgramError::MathOverflow)?;
    let slot = Clock::get()?.slot;
    let market_key = *ix.accounts.market.address();

    reset_round_to_collect(
        program_id,
        ix.accounts.market,
        ix.accounts.histogram,
        num_ticks,
        next_id,
        slot,
    )?;

    // Stage A sharding: forcibly zero + re-arm the passed shard. Unlike `reset_shard`
    // (which requires a drained, post-clearing shard), the escape hatch must clear a
    // *wedged* shard whose orders were never settled — so there is no `count == 0` or
    // phase gate here. A multi-shard market must `force_reset` each dirty shard.
    {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;
        {
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
        }
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

    Ok(())
}
