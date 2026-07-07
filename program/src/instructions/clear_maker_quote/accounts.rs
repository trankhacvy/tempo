use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ClearMakerQuote instruction (zero the ladder + deactivate).
///
/// # Account Layout
/// 0. `[signer]` writer - the maker or its delegate
/// 1. `[writable]` market - decrements `active_maker_quote_count`
/// 2. `[writable]` maker_quote
/// 3. `[writable]` user_collateral (OPTIONAL) - the MAKER's ledger; the ladder's
///    standing reservation is released here in full (missing-features §7.1).
///    REQUIRED whenever the quote carries a non-zero reservation; a clearing-only
///    market (which can never have one) omits it.
pub struct ClearMakerQuoteAccounts<'a> {
    pub writer: &'a AccountView,
    pub market: &'a AccountView,
    pub maker_quote: &'a AccountView,
    pub user_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for ClearMakerQuoteAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [writer, market, maker_quote, rest @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(writer, false)?;
        verify_writable(market, true)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(maker_quote)?;

        // Optional maker ledger (required iff the quote carries a reservation).
        let user_collateral = match rest {
            [] => None,
            [uc] => {
                verify_writable(uc, true)?;
                verify_current_program_account(uc)?;
                Some(uc)
            }
            _ => return Err(ProgramError::NotEnoughAccountKeys),
        };

        Ok(Self {
            writer,
            market,
            maker_quote,
            user_collateral,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ClearMakerQuoteAccounts<'a> {}
