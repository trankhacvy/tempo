use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the CloseMakerQuote instruction (reclaim a cleared quote's rent).
///
/// # Account Layout
/// 0. `[signer, writable]` maker - the quote's maker; receives the reclaimed rent
/// 1. `[writable]` maker_quote - the inactive MakerQuote PDA to close
pub struct CloseMakerQuoteAccounts<'a> {
    pub maker: &'a AccountView,
    pub maker_quote: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for CloseMakerQuoteAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [maker, maker_quote] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(maker, true)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(maker_quote)?;

        Ok(Self { maker, maker_quote })
    }
}

impl<'a> InstructionAccounts<'a> for CloseMakerQuoteAccounts<'a> {}
