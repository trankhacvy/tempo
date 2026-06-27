use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_system_program, verify_writable},
};

/// Accounts for the InitCollateral instruction.
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[signer]` owner - the trader the ledger belongs to
/// 2. `[writable]` user_collateral - PDA to create
/// 3. `[]` system_program
pub struct InitCollateralAccounts<'a> {
    pub payer: &'a AccountView,
    pub owner: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitCollateralAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, owner, user_collateral, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_signer(owner, false)?;
        verify_writable(user_collateral, true)?;
        verify_system_program(system_program)?;

        Ok(Self {
            payer,
            owner,
            user_collateral,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitCollateralAccounts<'a> {}
