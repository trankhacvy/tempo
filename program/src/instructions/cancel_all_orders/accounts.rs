use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the CancelAllOrders instruction (missing-features §2.7).
///
/// # Account Layout — mirrors CancelOrder exactly
/// 0. `[signer]` trader — the OWNER whose orders are cancelled. Unlike
///    `cancel_order` there is NO reaper path here: a batch cancel only ever
///    touches the signer's own orders (reaping strangers' expired orders stays
///    on the single-order instruction, where the strict-`<` boundary lives).
/// 1. `[]` market — read-only (Design Z: cancel writes no shared account)
/// 2. `[writable]` order_slab — the ONE shard scanned (multi-shard = client loop)
/// 3. `[]` event_authority - Event authority PDA
/// 4. `[]` tempo_program - Current program
/// 5. `[writable]` user_collateral *(optional)* - the trader's collateral ledger
///
/// The trailing `user_collateral` is REQUIRED whenever the summed
/// `reserved_margin` of the cancelled orders is non-zero (a money-path market):
/// the batch releases ONE summed reservation back to the owner's free balance.
pub struct CancelAllOrdersAccounts<'a> {
    pub trader: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub user_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for CancelAllOrdersAccounts<'a> {
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

impl<'a> InstructionAccounts<'a> for CancelAllOrdersAccounts<'a> {}
