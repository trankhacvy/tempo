use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_system_program, verify_writable},
};

/// Accounts for the InitVault instruction (admin).
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[signer]` admin
/// 2. `[writable]` vault - the Vault singleton PDA to create
/// 3. `[]` vault_token_account - SPL token account owned by the vault authority
/// 4. `[]` collateral_mint
/// 5. `[]` system_program
pub struct InitVaultAccounts<'a> {
    pub payer: &'a AccountView,
    pub admin: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub collateral_mint: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitVaultAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, admin, vault, vault_token_account, collateral_mint, system_program] = accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_signer(admin, false)?;
        verify_writable(vault, true)?;
        verify_system_program(system_program)?;

        Ok(Self {
            payer,
            admin,
            vault,
            vault_token_account,
            collateral_mint,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitVaultAccounts<'a> {}
