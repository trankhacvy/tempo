use pinocchio::{
    account::AccountView,
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::UpdateMakerQuoteLevels,
    state::{require_quote_writer, AuctionPhase, MakerQuote, Market},
    traits::{AccountDeserialize, PdaAccount},
};

/// Reads a level's tick offset from a flat ladder region.
#[inline(always)]
fn level_offset(region: &[u8], i: usize) -> u32 {
    let base = i * 10;
    u16::from_le_bytes([region[base], region[base + 1]]) as u32
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

    let num_ticks = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Collect)?;
        market.num_ticks()
    };
    let mid = ix.data.mid_tick;
    if mid >= num_ticks {
        return Err(TempoProgramError::InvalidTick.into());
    }
    for i in 0..ix.data.num_bids as usize {
        if level_offset(&ix.data.bid_levels, i) > mid {
            return Err(TempoProgramError::InvalidTick.into());
        }
    }
    for i in 0..ix.data.num_asks as usize {
        let tick = mid
            .checked_add(level_offset(&ix.data.ask_levels, i))
            .ok_or(TempoProgramError::InvalidTick)?;
        if tick >= num_ticks {
            return Err(TempoProgramError::InvalidTick.into());
        }
    }

    let now = Clock::get()?.slot;
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

    quote.set_mid_tick(mid);
    quote.set_sequence(ix.data.sequence);
    quote.set_last_update_slot(now);
    quote.num_bids = ix.data.num_bids;
    quote.num_asks = ix.data.num_asks;
    quote.bid_levels_le.copy_from_slice(&ix.data.bid_levels);
    quote.ask_levels_le.copy_from_slice(&ix.data.ask_levels);
    Ok(())
}
