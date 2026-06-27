use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_writable},
};

/// Accounts for the AddPositionToMargin instruction (cross-margin).
///
/// # Account Layout
/// 0. `[signer]` owner - must own both the group and the position
/// 1. `[writable]` margin_account - the group to extend
/// 2. `[writable]` position - a flat, ungrouped position to bind
/// 3. `[]` market - the position's market (binds the order slab)
/// 4. `[]` order_slab - scanned to reject binding with an in-flight order
pub struct AddPositionToMarginAccounts<'a> {
    pub owner: &'a AccountView,
    pub margin_account: &'a AccountView,
    pub position: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for AddPositionToMarginAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [owner, margin_account, position, market, order_slab] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, false)?;
        verify_writable(margin_account, true)?;
        verify_writable(position, true)?;

        Ok(Self {
            owner,
            margin_account,
            position,
            market,
            order_slab,
        })
    }
}

impl<'a> InstructionAccounts<'a> for AddPositionToMarginAccounts<'a> {}
