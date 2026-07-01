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
/// 3. `[writable]` clearing_result - PDA to create
/// 4. `[]` system_program
/// 5. `[]` event_authority - Event authority PDA
/// 6. `[]` tempo_program - Current program
/// 7. `[writable]` cranker_collateral - (OPTIONAL, sentinel = program id) cranker's
///    collateral ledger; when present with `vault`, the flat crank fee is paid into it.
/// 8. `[writable]` vault - (OPTIONAL, sentinel = program id) fee/insurance pool.
/// 9. `[]` order_slab shards (×`num_slab_shards`) — ALL of the market's shards, read-only. Design Z (DDR-1):
///    completeness is proven by scanning every shard here (`all_active_orders_accumulated`
///    per shard), so the caller MUST pass every shard (`shards.len() == num_slab_shards`,
///    enforced in the processor); a short/wrong/duplicate set is rejected. The crank-fee
///    accounts hold fixed slots 7/8 (program-id sentinels when omitted) so the shard region
///    starts at a deterministic offset.
///
/// (Account-limit ceiling: a market with more shards than fit in one transaction's account
/// list cannot be finalized in a single tx — size shard counts accordingly; see DDR-1.)
pub struct FinalizeClearAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub clearing_result: &'a AccountView,
    pub system_program: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub cranker_collateral: Option<&'a AccountView>,
    pub vault: Option<&'a AccountView>,
    pub shards: &'a [AccountView],
}

impl<'a> TryFrom<&'a [AccountView]> for FinalizeClearAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, histogram, clearing_result, system_program, event_authority, tempo_program, cranker_collateral, vault, shards @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        // At least one shard must follow the (sentinel-filled) crank-fee slots.
        if shards.is_empty() {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        verify_signer(cranker, true)?;
        verify_writable(market, true)?;
        verify_writable(clearing_result, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_system_program(system_program)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        // Codama fills an omitted optional account with the program id as a sentinel, so an
        // optional account whose address == the program id is treated as "not provided".
        let present = |a: &&'a AccountView| a.address() != &crate::ID;
        let cranker_collateral = Some(cranker_collateral).filter(present);
        if let Some(cc) = cranker_collateral {
            verify_writable(cc, true)?;
            verify_current_program_account(cc)?;
        }
        let vault = Some(vault).filter(present);
        if let Some(v) = vault {
            verify_writable(v, true)?;
            verify_current_program_account(v)?;
        }

        // Shard accounts are READ-ONLY here — finalize only scans them for completeness.
        for shard in shards.iter() {
            verify_current_program_account(shard)?;
        }

        Ok(Self {
            cranker,
            market,
            histogram,
            clearing_result,
            system_program,
            event_authority,
            tempo_program,
            cranker_collateral,
            vault,
            shards,
        })
    }
}

impl<'a> InstructionAccounts<'a> for FinalizeClearAccounts<'a> {}
