use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::{reset_round_to_collect, ForceReset},
    state::Market,
};

/// Processes the ForceReset instruction — an authority-gated escape hatch that
/// abandons a wedged round and reopens `Collect`, regardless of phase or
/// unsettled orders (system-design §7). It bumps the auction id, zeroes the
/// histogram, resets the counters + shard aggregates, and opens a fresh collection
/// window. This is an operational backstop for a stuck round, NOT a normal path — the
/// permissionless cranks drain a round under the freeze model on their own.
///
/// Stage A sharding: this does NOT zero the slab shards (a market may have too many for
/// one tx). It re-arms `shards_pending`/`shards_ready` for the new round, so the admin
/// must `reset_shard` each dirty shard afterward before the next `start_auction`.
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

    reset_round_to_collect(
        program_id,
        ix.accounts.market,
        ix.accounts.histogram,
        num_ticks,
        next_id,
        slot,
    )
}
