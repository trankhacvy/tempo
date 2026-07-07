use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_token_program, verify_writable},
};

/// Accounts for the Withdraw instruction.
///
/// # Account Layout
/// 0. `[signer]` owner
/// 1. `[writable]` user_collateral - owner's ledger
/// 2. `[]` vault
/// 3. `[]` vault_authority - PDA that owns the vault token account
/// 4. `[writable]` vault_token_account
/// 5. `[writable]` user_token_account
/// 6. `[]` token_program
pub struct WithdrawAccounts<'a> {
    pub owner: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_authority: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub user_token_account: &'a AccountView,
    pub token_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for WithdrawAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, user_collateral, vault, vault_authority, vault_token_account, user_token_account, token_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, false)?;
        verify_writable(user_collateral, true)?;
        verify_current_program_account(user_collateral)?;
        verify_writable(vault, true)?;
        verify_current_program_account(vault)?;
        verify_writable(vault_token_account, true)?;
        verify_writable(user_token_account, true)?;
        // HS-12: only the canonical SPL Token program (no transfer-fee mints).
        verify_token_program(token_program)?;

        Ok(Self {
            owner,
            user_collateral,
            vault,
            vault_authority,
            vault_token_account,
            user_token_account,
            token_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for WithdrawAccounts<'a> {}
