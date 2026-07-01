use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_current_program_account, verify_event_authority,
        verify_signer, verify_writable,
    },
};

/// Accounts for the SubmitOrder instruction.
///
/// # Account Layout
/// 0. `[signer, writable]` trader
/// 1. `[]` market
/// 2. `[writable]` order_slab — the chosen shard `[b"order_slab", market, shard_id]`
///    (Stage A sharding); the processor validates its PDA against `data.shard_id`
/// 3. `[]` event_authority - Event authority PDA
/// 4. `[]` tempo_program - Current program
/// 5. `[writable]` position *(optional)* - the trader's position for this market
/// 6. `[writable]` user_collateral *(optional)* - the trader's collateral ledger
///
/// The trailing `position` + `user_collateral` are REQUIRED on a money-path market
/// (`maintenance_margin_bps > 0`): the processor reserves the order's worst-case
/// initial margin into the ledger so a matched trade can always settle
/// (missing-features §1.1). A no-money-path clearing market omits them.
pub struct SubmitOrderAccounts<'a> {
    pub trader: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    pub position: Option<&'a AccountView>,
    pub user_collateral: Option<&'a AccountView>,
}

impl<'a> TryFrom<&'a [AccountView]> for SubmitOrderAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [trader, market, order_slab, event_authority, tempo_program, rest @ ..] = accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(trader, true)?;
        // Stage A: `market` is writable — the FIRST order into an empty shard bumps
        // `Market.shards_pending` (the completeness aggregate that excludes empty shards).
        verify_writable(market, true)?;
        verify_writable(order_slab, true)?;

        // Both state accounts must be owned by this program.
        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        // Optional money-path accounts: present together or not at all.
        let (position, user_collateral) = match rest {
            [] => (None, None),
            [position, user_collateral] => {
                // `position` is read-only here (only its size is read to size the
                // reservation); `user_collateral` is written (margin is locked).
                verify_writable(user_collateral, true)?;
                verify_current_program_account(position)?;
                verify_current_program_account(user_collateral)?;
                (Some(position), Some(user_collateral))
            }
            _ => return Err(ProgramError::NotEnoughAccountKeys),
        };

        Ok(Self {
            trader,
            market,
            order_slab,
            event_authority,
            tempo_program,
            position,
            user_collateral,
        })
    }
}

impl<'a> InstructionAccounts<'a> for SubmitOrderAccounts<'a> {}
