use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the UpdateMakerQuoteLevels instruction (full ladder rewrite).
///
/// # Account Layout
/// 0. `[signer]` writer - the maker or its delegate
/// 1. `[]` market - supplies `num_ticks` for the bound checks
/// 2. `[writable]` maker_quote
pub struct UpdateMakerQuoteLevelsAccounts<'a> {
    pub writer: &'a AccountView,
    pub market: &'a AccountView,
    pub maker_quote: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for UpdateMakerQuoteLevelsAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [writer, market, maker_quote] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(writer, false)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(maker_quote)?;

        Ok(Self {
            writer,
            market,
            maker_quote,
        })
    }
}

impl<'a> InstructionAccounts<'a> for UpdateMakerQuoteLevelsAccounts<'a> {}
