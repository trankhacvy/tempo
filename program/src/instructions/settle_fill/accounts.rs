use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the SettleFill instruction (permissionless to trigger).
///
/// # Account Layout
/// 0. `[signer]` cranker
/// 1. `[writable]` market
/// 2. `[writable]` order_slab
/// 3. `[]` clearing_result
/// 4. `[]` event_authority - Event authority PDA
/// 5. `[]` tempo_program - Current program
/// 6. `[writable]` position - order owner's Position. REQUIRED whenever the
///    settle produces a non-zero fill (the matched trade is applied to it, so it
///    can never be silently discarded); omittable only for a zero-fill order.
///    Enforced in the processor.
/// 7. `[writable]` user_collateral - owner's collateral ledger. REQUIRED on a
///    non-zero fill when the market is margin-enabled (maintenance_margin_bps > 0);
///    optional only for a no-margin market. When supplied, funding-realized PnL is
///    flushed and margin is re-locked to the new size on this fill.
/// 8. `[]` vault - (OPTIONAL) fee/insurance pool; REQUIRED when a protocol fee
///    applies to this fill.
/// 9. `[writable]` integrator_collateral - (OPTIONAL) an integrator's collateral
///    ledger; when supplied, receives `integrator_share_bps` of a positive fee.
pub struct SettleFillAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub clearing_result: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub position: Option<&'a AccountView>,
    pub user_collateral: Option<&'a AccountView>,
    pub vault: Option<&'a AccountView>,
    pub integrator_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for SettleFillAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, order_slab, clearing_result, event_authority, tempo_program, rest @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;
        verify_current_program_account(clearing_result)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        // Codama fills an omitted optional account with the program id as a
        // sentinel, so any optional account whose address == the program id is
        // treated as "not provided".
        let present = |a: &&'a AccountView| a.address() != &crate::ID;
        let position = rest.first().filter(present);
        if let Some(p) = position {
            verify_writable(p, true)?;
            verify_current_program_account(p)?;
        }
        let user_collateral = rest.get(1).filter(present);
        if let Some(uc) = user_collateral {
            verify_writable(uc, true)?;
            verify_current_program_account(uc)?;
        }
        let vault = rest.get(2).filter(present);
        if let Some(v) = vault {
            verify_current_program_account(v)?;
        }
        let integrator_collateral = rest.get(3).filter(present);
        if let Some(ic) = integrator_collateral {
            verify_writable(ic, true)?;
            verify_current_program_account(ic)?;
        }

        Ok(Self {
            cranker,
            market,
            order_slab,
            clearing_result,
            event_authority,
            tempo_program,
            position,
            user_collateral,
            vault,
            integrator_collateral,
        })
    }
}

impl<'a> InstructionAccounts<'a> for SettleFillAccounts<'a> {}
