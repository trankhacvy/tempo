use pinocchio::{account::AccountView, Address, ProgramResult};
use pinocchio_token::instructions::Transfer;

use crate::{
    errors::TempoProgramError,
    instructions::Deposit,
    state::{UserCollateral, Vault},
    traits::AccountDeserialize,
};

/// Processes the Deposit instruction: transfers `amount`
/// collateral from the owner's token account into the vault token account, then
/// credits the owner's `UserCollateral` ledger. The owner signs the SPL transfer.
pub fn process_deposit(
    _program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = Deposit::try_from((instruction_data, accounts))?;
    let amount = ix.data.amount;

    // The provided vault token account must be the one recorded in the vault.
    {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // The ledger must belong to the signing owner.
    {
        let uc_data = ix.accounts.user_collateral.try_borrow()?;
        let uc = UserCollateral::from_bytes(&uc_data)?;
        if uc.owner != *ix.accounts.owner.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // Move tokens (owner-signed). All program account borrows are dropped above.
    Transfer::new(
        ix.accounts.user_token_account,
        ix.accounts.vault_token_account,
        ix.accounts.owner,
        amount,
    )
    .invoke()?;

    // Credit the ledger.
    {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        uc.credit(amount)?;
    }

    Ok(())
}
