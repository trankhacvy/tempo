use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the ProcessChunk instruction (permissionless).
///
/// # Account Layout
/// 0. `[signer, writable]` cranker
/// 1. `[writable]` market
/// 2. `[writable]` order_slab
/// 3. `[writable]` histogram
/// 4. `[]` event_authority - Event authority PDA
/// 5. `[]` tempo_program - Current program
pub struct ProcessChunkAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub histogram: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ProcessChunkAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, order_slab, histogram, event_authority, tempo_program] = accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, true)?;
        verify_writable(market, true)?;
        verify_writable(order_slab, true)?;
        verify_writable(histogram, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;
        verify_current_program_account(histogram)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            cranker,
            market,
            order_slab,
            histogram,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ProcessChunkAccounts<'a> {}
