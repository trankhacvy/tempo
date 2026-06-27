use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::InitPosition,
    state::{Market, Position},
    traits::{AccountSerialize, AccountSize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes the InitPosition instruction: creates an empty
/// `Position` PDA for `(market, owner)` so the owner's fills can be applied to
/// it during `settle_fill`. Funding starts at the market's current funding index.
pub fn process_init_position(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitPosition::try_from((instruction_data, accounts))?;

    // Validate the market belongs to this program and capture the live funding
    // index so the position opens at the current checkpoint, not 0.
    let funding_index = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.funding_index()
    };

    let owner = *ix.accounts.owner.address();
    let market_key = *ix.accounts.market.address();
    let position = Position::new(ix.data.position_bump, owner, market_key, funding_index);
    position.validate_pda(ix.accounts.position, program_id, ix.data.position_bump)?;

    let bump = [ix.data.position_bump];
    let seeds: Vec<Seed> = position.seeds_with_bump(&bump);
    let seeds_array: [Seed; 4] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.payer,
        Position::LEN,
        program_id,
        ix.accounts.position,
        seeds_array,
    )?;

    {
        let mut acct = *ix.accounts.position;
        let mut slice = acct.try_borrow_mut()?;
        position.write_to_slice(&mut slice)?;
    }

    Ok(())
}
