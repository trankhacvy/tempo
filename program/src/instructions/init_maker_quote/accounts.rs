use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program_account, verify_signer, verify_system_program, verify_writable,
    },
};

/// Accounts for the InitMakerQuote instruction.
///
/// # Account Layout
/// 0. `[signer, writable]` maker - pays rent and owns the quote
/// 1. `[writable]` market - bumps `next_quote_id` + `active_maker_quote_count`
/// 2. `[writable]` maker_quote - PDA to create
/// 3. `[]` system_program
pub struct InitMakerQuoteAccounts<'a> {
    pub maker: &'a AccountView,
    pub market: &'a AccountView,
    pub maker_quote: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitMakerQuoteAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [maker, market, maker_quote, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(maker, true)?;
        verify_writable(market, true)?;
        verify_writable(maker_quote, true)?;

        verify_current_program_account(market)?;
        verify_system_program(system_program)?;

        Ok(Self {
            maker,
            market,
            maker_quote,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitMakerQuoteAccounts<'a> {}
