use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::ProcessMakerQuote,
    state::{
        fold, read_region, AuctionHistogramHeader, AuctionPhase, MakerQuote, Market, Region,
        MAX_LEVELS, SNAPSHOT_UNFOLDED,
    },
    traits::{AccountDeserialize, PdaAccount, PdaSeeds},
};

/// Processes ProcessMakerQuote — Phase 1 ACCUMULATE for a single persistent maker
/// quote. Folds the quote's ladder into the histogram exactly once per round
/// (commutative, like an order), tags it, and bumps the market's
/// `folded_maker_quote_count` so `finalize_clear` can verify maker completeness.
/// Inactive or already-folded quotes are a no-op; an expired-but-active quote
/// folds zero yet is still tagged + counted so it cannot block finalization.
pub fn process_process_maker_quote(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProcessMakerQuote::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- read market params + phase gate (mirrors process_chunk) ---
    let (num_ticks, auction_id) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        let phase = market.phase()?;
        if phase != AuctionPhase::Collect && phase != AuctionPhase::Accumulating {
            return Err(TempoProgramError::AuctionWrongPhase.into());
        }
        (market.num_ticks(), market.current_auction_id())
    };

    // Transition Collect -> Accumulating once the window closes.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        if market.phase == AuctionPhase::Collect as u8 {
            if Clock::get()?.slot < market.phase_deadline_slot() {
                return Err(TempoProgramError::AuctionWindowOpen.into());
            }
            market.phase = AuctionPhase::Accumulating as u8;
        }
    }

    let now = Clock::get()?.slot;
    let mut quote_account = *ix.accounts.maker_quote;
    let mut quote_data = quote_account.try_borrow_mut()?;
    // Copy the ladder into locals so we can fold (immutable histogram read) and then
    // write the per-level snapshots back into the quote without an aliasing borrow.
    let mut bid_lv = [(0u16, 0u64); MAX_LEVELS];
    let mut ask_lv = [(0u16, 0u64); MAX_LEVELS];
    let (mid_tick, n_bid, n_ask, status, expired, folded_aid) = {
        let quote = MakerQuote::from_bytes(&quote_data)?;
        if quote.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        quote.validate_self(ix.accounts.maker_quote, program_id)?;
        let n_bid = quote.num_bids as usize;
        let n_ask = quote.num_asks as usize;
        for (i, slot) in bid_lv.iter_mut().enumerate().take(n_bid) {
            *slot = quote.bid_level(i);
        }
        for (i, slot) in ask_lv.iter_mut().enumerate().take(n_ask) {
            *slot = quote.ask_level(i);
        }
        (
            quote.mid_tick(),
            quote.num_bids,
            quote.num_asks,
            quote.status,
            quote.is_expired(now),
            quote.folded_auction_id(),
        )
    };

    // Inactive (not counted) or already folded this round → idempotent no-op.
    if status != 1 || folded_aid == auction_id {
        return Ok(());
    }

    // Per-level `cum_before` captured during the fold. Defaults to the unfolded
    // sentinel so an expired quote (folds nothing) or an off-grid level fills zero
    // in settlement — never a phantom fill from a level the histogram never saw.
    let mut bid_snap = [SNAPSHOT_UNFOLDED; MAX_LEVELS];
    let mut ask_snap = [SNAPSHOT_UNFOLDED; MAX_LEVELS];

    // Fold the ladder (an expired-but-active quote folds zero, but is still tagged).
    if !expired {
        let mut hist_account = *ix.accounts.histogram;
        let mut hist_data = hist_account.try_borrow_mut()?;
        {
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
        // Maker bids fold into BidDemand (maker buy); asks into AskSupply (maker sell).
        // A mid move can push a level off the grid; skip it (settle does the same).
        // For each folded level, record the bucket value BEFORE this fold: since only
        // maker quotes feed BidDemand/AskSupply (taker orders go to the taker regions
        // post-§1.3), that is exactly this quote's `cum_before` among the makers at
        // the tick — the conserving telescoping prefix (§1.6).
        for (i, &(offset, size)) in bid_lv.iter().enumerate().take(n_bid as usize) {
            let Some(tick) = mid_tick.checked_sub(offset as u32) else {
                continue;
            };
            if tick >= num_ticks {
                continue;
            }
            bid_snap[i] = read_region(&hist_data, num_ticks, Region::BidDemand, tick)?;
            fold(&mut hist_data, num_ticks, Region::BidDemand, tick, size)?;
        }
        for (i, &(offset, size)) in ask_lv.iter().enumerate().take(n_ask as usize) {
            let Some(tick) = mid_tick
                .checked_add(offset as u32)
                .filter(|t| *t < num_ticks)
            else {
                continue;
            };
            ask_snap[i] = read_region(&hist_data, num_ticks, Region::AskSupply, tick)?;
            fold(&mut hist_data, num_ticks, Region::AskSupply, tick, size)?;
        }
    }

    // Persist the snapshots (always reset first so a prior round's prefixes can't
    // leak into a now-unfolded level) and tag folded-this-round (idempotency marker).
    {
        let quote = MakerQuote::from_bytes_mut(&mut quote_data)?;
        quote.reset_snapshots();
        for (i, &cum) in bid_snap.iter().enumerate().take(n_bid as usize) {
            quote.set_bid_snapshot(i, cum);
        }
        for (i, &cum) in ask_snap.iter().enumerate().take(n_ask as usize) {
            quote.set_ask_snapshot(i, cum);
        }
        quote.set_folded_auction_id(auction_id);
    }

    // Bump the maker-quote completeness counter.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_folded_maker_quote_count(
            market
                .folded_maker_quote_count()
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
    }

    Ok(())
}
