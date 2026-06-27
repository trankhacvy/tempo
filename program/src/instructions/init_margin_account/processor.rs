use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::InitMarginAccount,
    state::MarginAccount,
    traits::{AccountSerialize, AccountSize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes InitMarginAccount: creates an empty cross-margin group PDA for
/// `owner` so member positions can later be bound to it.
pub fn process_init_margin_account(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitMarginAccount::try_from((instruction_data, accounts))?;

    let owner = *ix.accounts.owner.address();
    let margin = MarginAccount::new(ix.data.margin_bump, owner);
    margin.validate_pda(ix.accounts.margin_account, program_id, ix.data.margin_bump)?;

    let bump = [ix.data.margin_bump];
    let seeds: Vec<Seed> = margin.seeds_with_bump(&bump);
    let seeds_array: [Seed; 3] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.payer,
        MarginAccount::LEN,
        program_id,
        ix.accounts.margin_account,
        seeds_array,
    )?;

    {
        let mut acct = *ix.accounts.margin_account;
        let mut slice = acct.try_borrow_mut()?;
        margin.write_to_slice(&mut slice)?;
    }

    Ok(())
}
