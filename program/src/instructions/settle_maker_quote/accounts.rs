use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the SettleMakerQuote instruction (permissionless to trigger).
///
/// # Account Layout
/// 0. `[signer]` cranker
/// 1. `[writable]` market
/// 2. `[]` clearing_result
/// 3. `[]` order_slab - scanned for orders sharing the marginal tick
/// 4. `[writable]` maker_quote - the quote to settle (tagged settled-this-round)
/// 5. `[writable]` position - the maker's Position
/// 6. `[writable]` user_collateral - (OPTIONAL) maker's ledger; REQUIRED when the
///    market is margin-enabled (maintenance_margin_bps > 0)
/// 7. `[writable]` vault - (OPTIONAL) fee/insurance pool; REQUIRED with a fee/PnL
pub struct SettleMakerQuoteAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub clearing_result: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub maker_quote: &'a AccountView,
    pub position: &'a AccountView,
    pub user_collateral: Option<&'a AccountView>,
    pub vault: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for SettleMakerQuoteAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, clearing_result, order_slab, maker_quote, position, rest @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(maker_quote, true)?;
        verify_writable(position, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(clearing_result)?;
        verify_current_program_account(order_slab)?;
        verify_current_program_account(maker_quote)?;
        verify_current_program_account(position)?;

        let present = |a: &&'a AccountView| a.address() != &crate::ID;
        let user_collateral = rest.first().filter(present);
        if let Some(uc) = user_collateral {
            verify_writable(uc, true)?;
            verify_current_program_account(uc)?;
        }
        let vault = rest.get(1).filter(present);
        if let Some(v) = vault {
            verify_writable(v, true)?;
            verify_current_program_account(v)?;
        }

        Ok(Self {
            cranker,
            market,
            clearing_result,
            order_slab,
            maker_quote,
            position,
            user_collateral,
            vault,
        })
    }
}

impl<'a> InstructionAccounts<'a> for SettleMakerQuoteAccounts<'a> {}
