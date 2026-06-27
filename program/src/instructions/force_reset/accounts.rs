use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ForceReset instruction (authority-gated escape hatch).
///
/// # Account Layout
/// 0. `[signer]` authority - must match `Market.authority`
/// 1. `[writable]` market
/// 2. `[writable]` histogram
/// 3. `[writable]` order_slab
pub struct ForceResetAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub order_slab: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ForceResetAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market, histogram, order_slab] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(authority, false)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_current_program_account(order_slab)?;

        Ok(Self {
            authority,
            market,
            histogram,
            order_slab,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ForceResetAccounts<'a> {}
