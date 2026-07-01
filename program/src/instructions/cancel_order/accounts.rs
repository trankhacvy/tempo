use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the CancelOrder instruction.
///
/// # Account Layout
/// 0. `[signer]` trader
/// 1. `[]` market — read-only (Design Z: cancel writes no shared account)
/// 2. `[writable]` order_slab
/// 3. `[]` event_authority - Event authority PDA
/// 4. `[]` tempo_program - Current program
/// 5. `[writable]` user_collateral *(optional)* - the trader's collateral ledger
///
/// The trailing `user_collateral` is REQUIRED whenever the cancelled order carries
/// a non-zero `reserved_margin` (a money-path market): cancelling releases that
/// worst-case reservation back to free balance (missing-features §1.1).
pub struct CancelOrderAccounts<'a> {
    pub trader: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub user_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for CancelOrderAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [trader, market, order_slab, event_authority, tempo_program, rest @ ..] = accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(trader, false)?;
        // Design Z (DDR-1): `market` is READ-ONLY — cancel writes only its own shard.
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        let user_collateral = match rest {
            [] => None,
            [user_collateral] => {
                verify_writable(user_collateral, true)?;
                verify_current_program_account(user_collateral)?;
                Some(user_collateral)
            }
            _ => return Err(ProgramError::NotEnoughAccountKeys),
        };

        Ok(Self {
            trader,
            market,
            order_slab,
            event_authority,
            tempo_program,
            user_collateral,
        })
    }
}

impl<'a> InstructionAccounts<'a> for CancelOrderAccounts<'a> {}
