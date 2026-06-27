use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ProcessMakerQuote instruction (permissionless crank).
///
/// # Account Layout
/// 0. `[signer]` cranker
/// 1. `[writable]` market - bumps `folded_maker_quote_count`
/// 2. `[writable]` histogram - the fold target
/// 3. `[writable]` maker_quote - the quote to fold (tagged folded-this-round)
pub struct ProcessMakerQuoteAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub maker_quote: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ProcessMakerQuoteAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, histogram, maker_quote] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_current_program_account(maker_quote)?;

        Ok(Self {
            cranker,
            market,
            histogram,
            maker_quote,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ProcessMakerQuoteAccounts<'a> {}
