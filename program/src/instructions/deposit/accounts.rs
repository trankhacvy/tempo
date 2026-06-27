use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the Deposit instruction.
///
/// # Account Layout
/// 0. `[signer]` owner
/// 1. `[writable]` user_collateral - owner's ledger
/// 2. `[]` vault
/// 3. `[writable]` vault_token_account
/// 4. `[writable]` user_token_account
/// 5. `[]` token_program
pub struct DepositAccounts<'a> {
    pub owner: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub user_token_account: &'a AccountView,
    pub token_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for DepositAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, user_collateral, vault, vault_token_account, user_token_account, token_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, false)?;
        verify_writable(user_collateral, true)?;
        verify_current_program_account(user_collateral)?;
        verify_current_program_account(vault)?;
        verify_writable(vault_token_account, true)?;
        verify_writable(user_token_account, true)?;

        Ok(Self {
            owner,
            user_collateral,
            vault,
            vault_token_account,
            user_token_account,
            token_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for DepositAccounts<'a> {}
