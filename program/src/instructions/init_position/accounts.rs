use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program_account, verify_signer, verify_system_program, verify_writable,
    },
};

/// Accounts for the InitPosition instruction.
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[signer]` owner - the trader the position belongs to
/// 2. `[]` market
/// 3. `[writable]` position - PDA to create
/// 4. `[]` system_program
pub struct InitPositionAccounts<'a> {
    pub payer: &'a AccountView,
    pub owner: &'a AccountView,
    pub market: &'a AccountView,
    pub position: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitPositionAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, owner, market, position, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_signer(owner, false)?;
        verify_writable(position, true)?;

        verify_current_program_account(market)?;
        verify_system_program(system_program)?;

        Ok(Self {
            payer,
            owner,
            market,
            position,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitPositionAccounts<'a> {}
