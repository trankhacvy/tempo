use pinocchio::{
    account::AccountView,
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::UpdateMakerQuoteLevels,
    margin::ladder_reservation,
    state::{require_quote_writer, AuctionPhase, MakerQuote, Market, UserCollateral},
    traits::{AccountDeserialize, PdaAccount},
};

/// Reads a level's tick offset from a flat ladder region.
#[inline(always)]
fn level_offset(region: &[u8], i: usize) -> u32 {
    let base = i * 10;
    u16::from_le_bytes([region[base], region[base + 1]]) as u32
}

/// Reads a level's base-lot size from a flat ladder region.
#[inline(always)]
fn level_size(region: &[u8], i: usize) -> u64 {
    let base = i * 10;
    u64::from_le_bytes(region[base + 2..base + 10].try_into().unwrap())
}

/// Anti-dust for one ladder level (missing-features §2.6): sized levels must meet
/// the market's minimum notional, priced conservatively at the WINDOW FLOOR (the
/// lowest possible in-window price) so the check is mid-independent — a later
/// `update_maker_quote_mid` can never dodge it. Zero-size levels are ignored.
#[inline(always)]
fn require_level_not_dust(
    size: u64,
    window_floor: u64,
    min_order_notional: u64,
) -> Result<(), pinocchio::error::ProgramError> {
    if min_order_notional > 0
        && size > 0
        && (size as u128) * (window_floor as u128) < min_order_notional as u128
    {
        return Err(TempoProgramError::OrderBelowMinimum.into());
    }
    Ok(())
}

/// Processes UpdateMakerQuoteLevels: rewrites the full ladder. Every bid level
/// must satisfy `offset <= mid_tick` (no underflow) and every ask level
/// `mid_tick + offset < num_ticks`, so folding can trust the ladder.
pub fn process_update_maker_quote_levels(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = UpdateMakerQuoteLevels::try_from((instruction_data, accounts))?;
    let writer = *ix.accounts.writer.address();
    let market_key = *ix.accounts.market.address();

    let (num_ticks, window_floor, min_order_notional, maintenance_bps, initial_bps, window_top) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Collect)?;
        // Circuit breaker (missing-features §3.2): the ladder freezes while paused.
        market.require_not_paused(Market::PAUSE_INTAKE)?;
        (
            market.num_ticks(),
            market.window_floor_price(),
            market.min_order_notional(),
            market.maintenance_margin_bps(),
            market.initial_margin_bps(),
            market.tick_to_price(market.num_ticks().saturating_sub(1))?,
        )
    };
    let mid = ix.data.mid_tick;
    if mid >= num_ticks {
        return Err(TempoProgramError::InvalidTick.into());
    }
    for i in 0..ix.data.num_bids as usize {
        if level_offset(&ix.data.bid_levels, i) > mid {
            return Err(TempoProgramError::InvalidTick.into());
        }
        require_level_not_dust(
            level_size(&ix.data.bid_levels, i),
            window_floor,
            min_order_notional,
        )?;
    }
    for i in 0..ix.data.num_asks as usize {
        let tick = mid
            .checked_add(level_offset(&ix.data.ask_levels, i))
            .ok_or(TempoProgramError::InvalidTick)?;
        if tick >= num_ticks {
            return Err(TempoProgramError::InvalidTick.into());
        }
        require_level_not_dust(
            level_size(&ix.data.ask_levels, i),
            window_floor,
            min_order_notional,
        )?;
    }

    // --- quote-time margin (missing-features §7.1) ---
    // Reserve the ladder's worst case NOW, so an unbacked ladder can never fold
    // into the histogram and steer the uniform clearing price for everyone (a
    // price-manipulation + insurance-drain vector). Mid-independent (priced at
    // the window top) so `update_maker_quote_mid` stays O(1) and collateral-free.
    let new_reserve = if maintenance_bps == 0 {
        0 // clearing-benchmark market: no money path, nothing to reserve
    } else {
        let mut total: u64 = 0;
        for i in 0..ix.data.num_bids as usize {
            total = total.saturating_add(level_size(&ix.data.bid_levels, i));
        }
        for i in 0..ix.data.num_asks as usize {
            total = total.saturating_add(level_size(&ix.data.ask_levels, i));
        }
        ladder_reservation(total, window_top, initial_bps)
    };

    let now = Clock::get()?.slot;
    let (old_reserve, quote_maker) = {
        let mut quote_account = *ix.accounts.maker_quote;
        let mut quote_data = quote_account.try_borrow_mut()?;
        let quote = MakerQuote::from_bytes_mut(&mut quote_data)?;
        if quote.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        quote.validate_self(ix.accounts.maker_quote, program_id)?;
        if quote.status != 1 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
        require_quote_writer(quote, &writer)?;
        if ix.data.sequence <= quote.sequence() {
            return Err(ProgramError::InvalidInstructionData);
        }
        let old_reserve = quote.reserved_margin();
        let quote_maker = quote.maker;

        quote.set_mid_tick(mid);
        quote.set_sequence(ix.data.sequence);
        quote.set_last_update_slot(now);
        quote.num_bids = ix.data.num_bids;
        quote.num_asks = ix.data.num_asks;
        quote.bid_levels_le.copy_from_slice(&ix.data.bid_levels);
        quote.ask_levels_le.copy_from_slice(&ix.data.ask_levels);
        quote.set_reserved_margin(new_reserve);
        quote.set_worst_price(window_top);
        (old_reserve, quote_maker)
    };

    // Delta-lock against the MAKER's ledger (a delegate reshapes the ladder
    // against the maker's collateral — it never moves funds anywhere else, and
    // an insufficient balance is a clean pre-trade rejection, same as
    // submit_order §1.1). Runs after the quote write: the tx is atomic, so a
    // failed lock reverts the ladder write with it.
    if new_reserve != old_reserve {
        // A reservation change REQUIRES the maker's ledger; a clearing-only
        // market never reaches here (maintenance == 0 → new_reserve == 0 ==
        // old_reserve, both always zero).
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        let mut uc = *uc_acct;
        let mut uc_data = uc.try_borrow_mut()?;
        let ledger = UserCollateral::from_bytes_mut(&mut uc_data)?;
        if ledger.owner != quote_maker {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        ledger.validate_self(uc_acct, program_id)?;
        if new_reserve > old_reserve {
            ledger.lock(new_reserve - old_reserve)?;
        } else {
            ledger.release(old_reserve - new_reserve);
        }
    }
    Ok(())
}
