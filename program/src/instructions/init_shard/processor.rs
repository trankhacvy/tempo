use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::InitShard,
    state::{Market, OrderSlabHeader},
    traits::{AccountSerialize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes the InitShard instruction (Stage A sharding).
///
/// Creates one `OrderSlab` shard `[b"order_slab", market, shard_id]` for the market.
/// A market has `num_slab_shards` shards, created one-per-tx (a market may have too many
/// for a single `initialize_market` tx, and this scales to `MAX_SLAB_SHARDS`). The shard
/// adopts the market's current auction id and per-shard cap.
///
/// Permissioned only by rent: anyone can create a market's shards (the address is
/// deterministic and the header fields are derived from the market), so a griefer can at
/// most pre-create the (correct) shards. Re-creating an existing shard fails
/// (`create_pda_account` is create-once).
pub fn process_init_shard(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitShard::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- read market: capacity, auction id, shard count (validates the market PDA) ---
    let (capacity, auction_id, num_slab_shards) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        (
            market.orders_per_auction_cap(),
            market.current_auction_id(),
            market.num_slab_shards(),
        )
    };

    if ix.data.shard_id >= num_slab_shards {
        return Err(TempoProgramError::ShardOutOfRange.into());
    }

    // --- build + create the shard PDA ---
    let shard = OrderSlabHeader::new(
        ix.data.bump,
        market_key,
        auction_id,
        capacity,
        ix.data.shard_id,
    );
    shard.validate_pda(ix.accounts.order_slab, program_id, ix.data.bump)?;

    let bump = [ix.data.bump];
    let seeds: Vec<Seed> = shard.seeds_with_bump(&bump);
    let seeds_array: [Seed; 4] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    let size = OrderSlabHeader::account_size(capacity);
    create_pda_account(
        ix.accounts.payer,
        size,
        program_id,
        ix.accounts.order_slab,
        seeds_array,
    )?;
    {
        // Slots are zero-initialized (status 0 == Empty); only the header needs writing.
        let mut slab_account = *ix.accounts.order_slab;
        let mut slice = slab_account.try_borrow_mut()?;
        shard.write_to_slice(&mut slice)?;
    }

    Ok(())
}
