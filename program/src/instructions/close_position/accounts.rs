use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ClosePosition instruction (reclaim a flat position's rent —
/// missing-features §3.4).
///
/// # Account Layout
/// 0. `[signer, writable]` owner - the position's owner; receives the reclaimed rent
/// 1. `[writable]` position - the flat, drained Position PDA to close
pub struct ClosePositionAccounts<'a> {
    pub owner: &'a AccountView,
    pub position: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ClosePositionAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, position] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, true)?;
        verify_writable(position, true)?;

        verify_current_program_account(position)?;

        Ok(Self { owner, position })
    }
}

impl<'a> InstructionAccounts<'a> for ClosePositionAccounts<'a> {}
