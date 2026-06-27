use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::InitCollateral,
    state::UserCollateral,
    traits::{AccountSerialize, AccountSize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes the InitCollateral instruction: creates an empty
/// `UserCollateral` ledger PDA for `owner`.
pub fn process_init_collateral(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitCollateral::try_from((instruction_data, accounts))?;

    let owner = *ix.accounts.owner.address();
    let user_collateral = UserCollateral::new(ix.data.bump, owner);
    user_collateral.validate_pda(ix.accounts.user_collateral, program_id, ix.data.bump)?;

    let bump = [ix.data.bump];
    let seeds: Vec<Seed> = user_collateral.seeds_with_bump(&bump);
    let seeds_array: [Seed; 3] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.payer,
        UserCollateral::LEN,
        program_id,
        ix.accounts.user_collateral,
        seeds_array,
    )?;

    {
        let mut acct = *ix.accounts.user_collateral;
        let mut slice = acct.try_borrow_mut()?;
        user_collateral.write_to_slice(&mut slice)?;
    }

    Ok(())
}
