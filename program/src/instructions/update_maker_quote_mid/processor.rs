use pinocchio::{
    account::AccountView,
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::UpdateMakerQuoteMid,
    state::{require_quote_writer, AuctionPhase, MakerQuote, Market},
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes UpdateMakerQuoteMid (O(1) re-quote): re-anchors the ladder by moving
/// `mid_tick`. Only the maker or its delegate may write, and the `sequence` must
/// strictly increase (replay guard).
pub fn process_update_maker_quote_mid(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = UpdateMakerQuoteMid::try_from((instruction_data, accounts))?;
    let writer = *ix.accounts.writer.address();
    let market_key = *ix.accounts.market.address();

    let num_ticks = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Collect)?;
        market.num_ticks()
    };
    if ix.data.mid_tick >= num_ticks {
        return Err(TempoProgramError::InvalidTick.into());
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

    quote.set_mid_tick(ix.data.mid_tick);
    quote.set_sequence(ix.data.sequence);
    quote.set_last_update_slot(now);
    Ok(())
}
