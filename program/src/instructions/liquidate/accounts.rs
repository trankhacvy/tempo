use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the Liquidate instruction (permissionless).
///
/// # Account Layout
/// 0. `[signer]` liquidator
/// 1. `[]` market
/// 2. `[]` oracle - the Pyth `PriceUpdateV2` account bound to the market
/// 3. `[writable]` position - the position being liquidated
/// 4. `[writable]` user_collateral - the position owner's ledger
/// 5. `[writable]` vault
/// 6. `[writable]` liquidator_collateral - the liquidator's ledger (paid the penalty)
/// 7. `[]` event_authority
/// 8. `[]` tempo_program
pub struct LiquidateAccounts<'a> {
    pub liquidator: &'a AccountView,
    pub market: &'a AccountView,
    pub oracle: &'a AccountView,
    pub position: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub vault: &'a AccountView,
    pub liquidator_collateral: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for LiquidateAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [liquidator, market, oracle, position, user_collateral, vault, liquidator_collateral, event_authority, tempo_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(liquidator, false)?;
        verify_current_program_account(market)?;
        // `oracle` ownership (Pyth receiver) is checked in the processor.
        verify_writable(position, true)?;
        verify_current_program_account(position)?;
        verify_writable(user_collateral, true)?;
        verify_current_program_account(user_collateral)?;
        verify_writable(vault, true)?;
        verify_current_program_account(vault)?;
        verify_writable(liquidator_collateral, true)?;
        verify_current_program_account(liquidator_collateral)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            liquidator,
            market,
            oracle,
            position,
            user_collateral,
            vault,
            liquidator_collateral,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for LiquidateAccounts<'a> {}
