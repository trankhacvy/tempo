use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_writable},
};

/// Accounts for the RemovePositionFromMargin instruction (cross-margin).
///
/// # Account Layout
/// 0. `[signer]` owner - must own both the group and the position
/// 1. `[writable]` margin_account - the group to shrink
/// 2. `[writable]` position - a flat member position to unbind
pub struct RemovePositionFromMarginAccounts<'a> {
    pub owner: &'a AccountView,
    pub margin_account: &'a AccountView,
    pub position: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for RemovePositionFromMarginAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, margin_account, position] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, false)?;
        verify_writable(margin_account, true)?;
        verify_writable(position, true)?;

        Ok(Self {
            owner,
            margin_account,
            position,
        })
    }
}

impl<'a> InstructionAccounts<'a> for RemovePositionFromMarginAccounts<'a> {}
