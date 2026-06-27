use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_system_program, verify_writable,
    },
};

/// Accounts for the FinalizeClear instruction (permissionless).
///
/// # Account Layout
/// 0. `[signer, writable]` cranker (paid a fee)
/// 1. `[writable]` market
/// 2. `[]` histogram
/// 3. `[]` order_slab - scanned for the slab-derived completeness gate
/// 4. `[writable]` clearing_result - PDA to create
/// 5. `[]` system_program
/// 6. `[]` event_authority - Event authority PDA
/// 7. `[]` tempo_program - Current program
/// 8. `[writable]` cranker_collateral - (OPTIONAL) cranker's collateral
///    ledger; when present with `vault`, the flat crank fee is paid into it.
/// 9. `[writable]` vault - (OPTIONAL) fee/insurance pool the crank fee is drawn from.
pub struct FinalizeClearAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub clearing_result: &'a AccountView,
    pub system_program: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub cranker_collateral: Option<&'a AccountView>,
    pub vault: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for FinalizeClearAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, histogram, order_slab, clearing_result, system_program, event_authority, tempo_program, rest @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, true)?;
        verify_writable(market, true)?;
        verify_writable(clearing_result, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_current_program_account(order_slab)?;
        verify_system_program(system_program)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        // Codama fills an omitted optional account with the program id as a
        // sentinel, so an optional account whose address == the program id is
        // treated as "not provided".
        let present = |a: &&'a AccountView| a.address() != &crate::ID;
        let cranker_collateral = rest.first().filter(present);
        if let Some(cc) = cranker_collateral {
            verify_writable(cc, true)?;
            verify_current_program_account(cc)?;
        }
        let vault = rest.get(1).filter(present);
        if let Some(v) = vault {
            verify_writable(v, true)?;
            verify_current_program_account(v)?;
        }

        Ok(Self {
            cranker,
            market,
            histogram,
            order_slab,
            clearing_result,
            system_program,
            event_authority,
            tempo_program,
            cranker_collateral,
            vault,
        })
    }
}

impl<'a> InstructionAccounts<'a> for FinalizeClearAccounts<'a> {}
