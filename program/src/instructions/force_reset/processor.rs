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
/// Stage A sharding: the caller passes ALL of the market's slab shards, and this zeroes
/// every one (the escape hatch can clear *wedged* shards that `reset_shard` cannot, since
/// `reset_shard` needs a drained, post-clearing shard). The auction id is bumped EXACTLY
/// ONCE — the round is reset atomically, so shards can never end at mismatched auction ids
/// (which would wedge the next roll's `reset_shard` id check). Every shard must be supplied
/// (`shards.len() == num_slab_shards`), so a partial reset can never leave a stale shard
/// holding the wedged round's orders to bleed into the next round's clearing.
pub fn process_force_reset(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ForceReset::try_from((instruction_data, accounts))?;

    // --- validate authority + capture params ---
    let (num_ticks, auction_id, num_slab_shards) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;
        (
            market.num_ticks(),
            market.current_auction_id(),
            market.num_slab_shards(),
        )
    };

    // Require EVERY shard in one call: an atomic reset is the whole point (bugs otherwise:
    // stale shards left behind, and per-call auction-id desync). This bounds force_reset to
    // markets whose shard count fits one tx's account list — documented on the accounts struct.
    if ix.accounts.shards.len() != num_slab_shards as usize {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }

    let next_id = auction_id
        .checked_add(1)
        .ok_or(TempoProgramError::MathOverflow)?;
    let slot = Clock::get()?.slot;
    let market_key = *ix.accounts.market.address();

    // Bump the round ONCE: zero the histogram, reset the roll gate (shards_ready = 0; Design Z has no shards_pending counter), reopen Collect at auction id `next_id`.
    reset_round_to_collect(
        program_id,
        ix.accounts.market,
        ix.accounts.histogram,
        num_ticks,
        next_id,
        slot,
    )?;

    // Forcibly zero + re-tag EVERY shard to the same `next_id`. No `count == 0` / phase gate
    // (unlike `reset_shard`): the escape hatch must clear shards whose orders were never
    // settled. Reject a duplicate shard_id so the caller can't satisfy the count check by
    // passing the same shard N times and leaving the real shards stale.
    let mut seen_mask: u64 = 0;
    for shard_ai in ix.accounts.shards {
        let mut slab_account = *shard_ai;
        let mut slab_data = slab_account.try_borrow_mut()?;
        let shard_id = {
            let slab = OrderSlabHeader::from_bytes(&slab_data)?;
            if slab.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            slab.validate_pda(shard_ai, program_id, slab.bump)?;
            slab.shard_id()
        };
        if shard_id >= num_slab_shards {
            return Err(TempoProgramError::ShardOutOfRange.into());
        }
        // De-dup: each shard index may appear at most once (mask covers up to 64 shards; for
        // larger markets this path is unusable anyway per the account-limit ceiling).
        let bit = 1u64
            .checked_shl(shard_id as u32)
            .ok_or(TempoProgramError::ShardOutOfRange)?;
        if seen_mask & bit != 0 {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        seen_mask |= bit;

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
