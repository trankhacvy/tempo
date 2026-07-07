use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the AcceptAuthorityTransfer instruction.
///
/// # Account Layout
/// 0. `[signer]` new_authority
/// 1. `[writable]` market
/// 2. `[]` event_authority
/// 3. `[]` tempo_program
pub struct AcceptAuthorityTransferAccounts<'a> {
    pub new_authority: &'a AccountView,
    pub market: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for AcceptAuthorityTransferAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [new_authority, market, event_authority, tempo_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(new_authority, false)?;
        verify_writable(market, true)?;
        verify_current_program_account(market)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            new_authority,
            market,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for AcceptAuthorityTransferAccounts<'a> {}
