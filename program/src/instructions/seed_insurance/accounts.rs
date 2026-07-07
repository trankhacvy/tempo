use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_token_program, verify_writable,
    },
};

/// Accounts for the SeedInsurance instruction (missing-features §4.1, the
/// permissionless donate half). Anyone may put money INTO the pool — there is no
/// attack in giving the vault money (both sides of the backing invariant grow
/// together). The withdraw half is authority-gated + staged and lands in Phase 3.
///
/// # Account Layout
/// 0. `[signer]` donor
/// 1. `[writable]` vault - insurance bookkeeping
/// 2. `[writable]` vault_token_account
/// 3. `[writable]` donor_token_account
/// 4. `[]` token_program - pinned classic SPL Token (HS-12)
/// 5. `[]` event_authority
/// 6. `[]` tempo_program
pub struct SeedInsuranceAccounts<'a> {
    pub donor: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub donor_token_account: &'a AccountView,
    pub token_program: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for SeedInsuranceAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [donor, vault, vault_token_account, donor_token_account, token_program, event_authority, tempo_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(donor, false)?;
        verify_writable(vault, true)?;
        verify_current_program_account(vault)?;
        verify_writable(vault_token_account, true)?;
        verify_writable(donor_token_account, true)?;
        // HS-12: only the canonical SPL Token program (no transfer-fee mints) —
        // insurance is credited by the face amount.
        verify_token_program(token_program)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            donor,
            vault,
            vault_token_account,
            donor_token_account,
            token_program,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for SeedInsuranceAccounts<'a> {}
