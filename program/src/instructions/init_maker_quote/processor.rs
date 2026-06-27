use alloc::vec::Vec;
use pinocchio::{
    account::AccountView,
    cpi::Seed,
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::InitMakerQuote,
    state::{AuctionPhase, MakerQuote, Market},
    traits::{AccountDeserialize, AccountSerialize, AccountSize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes InitMakerQuote: creates a persistent `MakerQuote` PDA for
/// `(market, maker)`, assigns it the market's next quote id, and counts it as an
/// active quote (the `finalize_clear` completeness denominator).
pub fn process_init_maker_quote(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitMakerQuote::try_from((instruction_data, accounts))?;
    let now = Clock::get()?.slot;
    let maker = *ix.accounts.maker.address();
    let market_key = *ix.accounts.market.address();

    // Validate the market and claim the next quote id + register as active.
    let quote_id = {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        {
            Market::from_account(&market_data, ix.accounts.market, program_id)?;
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.require_phase(AuctionPhase::Collect)?;
        let qid = market.next_quote_id();
        market.set_next_quote_id(qid.checked_add(1).ok_or(TempoProgramError::MathOverflow)?);
        market.set_active_maker_quote_count(
            market
                .active_maker_quote_count()
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        qid
    };

    let delegate = Address::new_from_array(ix.data.delegate);
    let quote = MakerQuote::new(
        ix.data.maker_quote_bump,
        maker,
        market_key,
        delegate,
        quote_id,
        ix.data.expiry_slots,
        now,
    );
    quote.validate_pda(
        ix.accounts.maker_quote,
        program_id,
        ix.data.maker_quote_bump,
    )?;

    let bump = [ix.data.maker_quote_bump];
    let seeds: Vec<Seed> = quote.seeds_with_bump(&bump);
    let seeds_array: [Seed; 4] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.maker,
        MakerQuote::LEN,
        program_id,
        ix.accounts.maker_quote,
        seeds_array,
    )?;

    {
        let mut acct = *ix.accounts.maker_quote;
        let mut slice = acct.try_borrow_mut()?;
        quote.write_to_slice(&mut slice)?;
    }

    Ok(())
}
