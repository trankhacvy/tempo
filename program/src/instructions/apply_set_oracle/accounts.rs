use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the ApplySetOracle instruction (permissionless after the delay).
///
/// # Account Layout
/// 0. `[signer]` cranker
/// 1. `[writable]` market
/// 2. `[]` new_oracle - the STAGED Pyth account, validated live before commit
/// 3. `[]` event_authority
/// 4. `[]` tempo_program
pub struct ApplySetOracleAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub new_oracle: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ApplySetOracleAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, new_oracle, event_authority, tempo_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_current_program_account(market)?;
        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            cranker,
            market,
            new_oracle,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ApplySetOracleAccounts<'a> {}
