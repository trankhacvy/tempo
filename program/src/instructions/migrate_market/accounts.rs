use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for MigrateMarket (admin-gated layout upgrade, VERSION 4 -> 5).
///
/// # Account layout
/// ```text
/// 0  [signer]            authority (must match Market.authority)
/// 1  [writable]          market    (the v4 account to upgrade in place)
/// 2  [signer, writable]  payer     (funds the rent for the grown account)
/// 3  []                  system_program
/// ```
pub struct MigrateMarketAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
    pub payer: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for MigrateMarketAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market, payer, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(authority, false)?;
        verify_writable(market, true)?;
        verify_signer(payer, true)?;
        verify_current_program_account(market)?;

        Ok(Self {
            authority,
            market,
            payer,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for MigrateMarketAccounts<'a> {}
