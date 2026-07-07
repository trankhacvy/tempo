use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ProposeAuthorityTransfer instruction (stage-only: no event until apply).
///
/// # Account Layout
/// 0. `[signer]` authority - must match `Market.authority`
/// 1. `[writable]` market
pub struct ProposeAuthorityTransferAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ProposeAuthorityTransferAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(authority, false)?;
        verify_writable(market, true)?;
        verify_current_program_account(market)?;

        Ok(Self { authority, market })
    }
}

impl<'a> InstructionAccounts<'a> for ProposeAuthorityTransferAccounts<'a> {}
