use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_token_program, verify_writable,
    },
};

/// Accounts for the ApplyInsuranceWithdraw instruction (permissionless after
/// the delay; the recipient token account is the authority's choice, recorded
/// at apply — the gate protects the USERS' backing, not the pool's destination).
///
/// # Account Layout
/// 0. `[signer]` cranker
/// 1. `[writable]` vault
/// 2. `[]` vault_authority (signs the token transfer)
/// 3. `[writable]` vault_token_account
/// 4. `[writable]` recipient_token_account (same mint, HS-12)
/// 5. `[]` token_program - pinned classic SPL Token
/// 6. `[]` event_authority
/// 7. `[]` tempo_program
pub struct ApplyInsuranceWithdrawAccounts<'a> {
    pub cranker: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_authority: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub recipient_token_account: &'a AccountView,
    pub token_program: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ApplyInsuranceWithdrawAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, vault, vault_authority, vault_token_account, recipient_token_account, token_program, event_authority, tempo_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(vault, true)?;
        verify_current_program_account(vault)?;
        verify_writable(vault_token_account, true)?;
        verify_writable(recipient_token_account, true)?;
        verify_token_program(token_program)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            cranker,
            vault,
            vault_authority,
            vault_token_account,
            recipient_token_account,
            token_program,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ApplyInsuranceWithdrawAccounts<'a> {}
