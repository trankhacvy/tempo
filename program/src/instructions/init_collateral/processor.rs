use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::InitCollateral,
    state::{UserCollateral, Vault},
    traits::{AccountDeserialize, AccountSerialize, AccountSize, PdaAccount, PdaSeeds},
    utils::create_pda_account,
};

/// Processes the InitCollateral instruction: creates an empty
/// `UserCollateral` ledger PDA for `(owner, vault.collateral_mint)`. The ledger is
/// mint-scoped (CR-3): its mint is read from the supplied vault and folded into the
/// PDA seeds, so a ledger can only ever back deposits/withdrawals against that mint.
pub fn process_init_collateral(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitCollateral::try_from((instruction_data, accounts))?;

    // The collateral mint is whatever the (program-owned) vault declares.
    let collateral_mint = {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        vault.collateral_mint
    };

    let owner = *ix.accounts.owner.address();
    let user_collateral = UserCollateral::new(ix.data.bump, owner, collateral_mint);
    user_collateral.validate_pda(ix.accounts.user_collateral, program_id, ix.data.bump)?;

    let bump = [ix.data.bump];
    let seeds: Vec<Seed> = user_collateral.seeds_with_bump(&bump);
    let seeds_array: [Seed; 4] = seeds
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
