use pinocchio::{account::AccountView, error::ProgramError, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::ClearMakerQuote,
    state::{require_quote_writer, AuctionPhase, MakerQuote, Market, LEVELS_LEN},
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes ClearMakerQuote: zeroes the ladder, marks the quote inactive, and
/// decrements the market's `active_maker_quote_count` so it no longer blocks
/// `finalize_clear`. (Rent reclaim via a future close instruction.)
pub fn process_clear_maker_quote(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ClearMakerQuote::try_from((instruction_data, accounts))?;
    let writer = *ix.accounts.writer.address();
    let market_key = *ix.accounts.market.address();

    {
        let mut quote_account = *ix.accounts.maker_quote;
        let mut quote_data = quote_account.try_borrow_mut()?;
        let quote = MakerQuote::from_bytes_mut(&mut quote_data)?;
        if quote.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        quote.validate_self(ix.accounts.maker_quote, program_id)?;
        require_quote_writer(quote, &writer)?;
        if quote.status != 1 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
        if ix.data.sequence <= quote.sequence() {
            return Err(ProgramError::InvalidInstructionData);
        }
        quote.set_sequence(ix.data.sequence);
        quote.num_bids = 0;
        quote.num_asks = 0;
        quote.status = 0;
        quote.bid_levels_le = [0u8; LEVELS_LEN];
        quote.ask_levels_le = [0u8; LEVELS_LEN];
    }

    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        {
            Market::from_account(&market_data, ix.accounts.market, program_id)?
                .require_phase(AuctionPhase::Collect)?;
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_active_maker_quote_count(market.active_maker_quote_count().saturating_sub(1));
    }

    Ok(())
}
