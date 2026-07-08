use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::CloseMarket,
    state::{AuctionHistogramHeader, AuctionPhase, ClearingResult, Market, OrderSlabHeader},
    traits::{AccountDeserialize, PdaSeeds},
    utils::close_pda_account,
};

/// Processes CloseMarket (missing-features §3.4): winds down a fully drained
/// market and reclaims the rent of EVERY per-market PDA — all shards, the
/// histogram, the clearing result, and finally the market itself — to the
/// authority.
///
/// This is deliberately the most-gated instruction in the program: closing a
/// market that still owes anyone anything must be impossible, so every gate
/// below is checked and each failure is the same loud `MarketNotQuiescent`
/// (except authority/shape mismatches, which keep their usual errors):
///
///  * **fully paused** (`paused == PAUSE_ALL`) — intake AND roll stopped, so no
///    new flow can race the close;
///  * **post-clearing phase** (Settling/Discovered) with **every shard reset**
///    (`shards_ready == num_slab_shards`) — no round is in flight;
///  * **zero open interest** on both sides — every position against this market
///    is flat (getting there is operational: pause intake, let closes/funding/
///    liquidation drain it; there is deliberately no force-close-at-oracle);
///  * **no active maker quotes** — every ladder was cleared (and its margin
///    released) via `clear_maker_quote`;
///  * **every shard empty** (`count == 0`) — no resting order (live or expired)
///    still carries a `reserved_margin` that closing its slab would strand.
///
/// The shard set is force_reset-style: ALL shards in one call, each exactly
/// once (count + dedup mask), so a market can never be closed while a stale
/// shard PDA survives it holding order state.
pub fn process_close_market(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = CloseMarket::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- authority + quiescence gates (read-only pass over the market) ---
    let num_slab_shards = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;

        let phase = market.phase()?;
        let quiescent = market.paused == Market::PAUSE_ALL
            && (phase == AuctionPhase::Settling || phase == AuctionPhase::Discovered)
            && market.shards_ready() == market.num_slab_shards()
            && market.oi_long() == 0
            && market.oi_short() == 0
            && market.active_maker_quote_count() == 0;
        if !quiescent {
            return Err(TempoProgramError::MarketNotQuiescent.into());
        }
        market.num_slab_shards()
    };

    // --- every shard, exactly once, all empty ---
    if ix.accounts.shards.len() != num_slab_shards as usize {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }
    let mut seen_mask: u64 = 0;
    for shard_ai in ix.accounts.shards {
        let slab_data = shard_ai.try_borrow()?;
        let slab = OrderSlabHeader::from_bytes(&slab_data)?;
        if slab.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        slab.validate_pda(shard_ai, program_id, slab.bump)?;
        let shard_id = slab.shard_id();
        if shard_id >= num_slab_shards {
            return Err(TempoProgramError::ShardOutOfRange.into());
        }
        let bit = 1u64
            .checked_shl(shard_id as u32)
            .ok_or(TempoProgramError::ShardOutOfRange)?;
        if seen_mask & bit != 0 {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        seen_mask |= bit;
        // No surviving order of ANY status: a resting order still carries its
        // owner's reserved margin; closing the slab would strand it.
        if slab.count() != 0 {
            return Err(TempoProgramError::MarketNotQuiescent.into());
        }
    }

    // --- histogram + clearing result must belong to this market ---
    {
        let hist_data = ix.accounts.histogram.try_borrow()?;
        let hist = AuctionHistogramHeader::from_bytes(&hist_data)?;
        if hist.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        hist.validate_pda(ix.accounts.histogram, program_id, hist.bump)?;
    }
    {
        let clearing_data = ix.accounts.clearing_result.try_borrow()?;
        let clearing = ClearingResult::from_bytes(&clearing_data)?;
        if clearing.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        clearing.validate_pda(ix.accounts.clearing_result, program_id, clearing.bump)?;
    }

    // --- close everything, rent to the authority; the market goes LAST so a
    // partial failure can never leave orphan PDAs behind a dead market ---
    for shard_ai in ix.accounts.shards {
        close_pda_account(shard_ai, ix.accounts.authority)?;
    }
    close_pda_account(ix.accounts.histogram, ix.accounts.authority)?;
    close_pda_account(ix.accounts.clearing_result, ix.accounts.authority)?;
    close_pda_account(ix.accounts.market, ix.accounts.authority)
}
