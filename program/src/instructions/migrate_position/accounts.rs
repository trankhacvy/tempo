use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for MigratePosition (owner-gated layout upgrade, VERSION 1|2 -> 3).
///
/// # Account layout
/// ```text
/// 0  [signer, writable]  owner   (must match Position.owner; pays the rent)
/// 1  [writable]          position (the v1/v2 account to upgrade in place)
/// 2  [writable]          market   (the v5 market; OI is rebuilt on a v1 upgrade)
/// 3  []                  order_slab (must be settled — quiescence gate for the OI rebuild)
/// 4  []                  system_program
/// ```
pub struct MigratePositionAccounts<'a> {
    pub owner: &'a AccountView,
    pub position: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for MigratePositionAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, position, market, order_slab, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, true)?;
        verify_writable(position, true)?;
        verify_writable(market, true)?;
        verify_current_program_account(position)?;
        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;

        Ok(Self {
            owner,
            position,
            market,
            order_slab,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for MigratePositionAccounts<'a> {}
