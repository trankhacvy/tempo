use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the UpdateMarketParams instruction.
///
/// # Account Layout
/// 0. `[signer]` authority
/// 1. `[writable]` market
/// 2. `[]` event_authority
/// 3. `[]` tempo_program
pub struct UpdateMarketParamsAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for UpdateMarketParamsAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market, event_authority, tempo_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(authority, false)?;
        verify_writable(market, true)?;
        verify_current_program_account(market)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            authority,
            market,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for UpdateMarketParamsAccounts<'a> {}
