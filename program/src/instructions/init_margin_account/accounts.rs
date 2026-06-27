use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_system_program, verify_writable},
};

/// Accounts for the InitMarginAccount instruction (cross-margin).
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[signer]` owner - the trader the group belongs to
/// 2. `[writable]` margin_account - PDA to create
/// 3. `[]` system_program
pub struct InitMarginAccountAccounts<'a> {
    pub payer: &'a AccountView,
    pub owner: &'a AccountView,
    pub margin_account: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitMarginAccountAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, owner, margin_account, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_signer(owner, false)?;
        verify_writable(margin_account, true)?;
        verify_system_program(system_program)?;

        Ok(Self {
            payer,
            owner,
            margin_account,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitMarginAccountAccounts<'a> {}
