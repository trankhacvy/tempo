use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the CloseMarket instruction (wind down a drained market and
/// reclaim every PDA's rent — missing-features §3.4).
///
/// # Account Layout
/// 0. `[signer, writable]` authority - must match `Market.authority`; receives ALL rent
/// 1. `[writable]` market
/// 2. `[writable]` histogram
/// 3. `[writable]` clearing_result
/// 4. `[writable]` order_slab shards (×`num_slab_shards`) — ALL of the market's
///    shards, force_reset-style (the processor rejects the call unless it
///    receives every shard exactly once), so a market can never be closed while
///    a stale shard PDA survives it.
pub struct CloseMarketAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub clearing_result: &'a AccountView,
    pub shards: &'a [AccountView],
}

impl<'a> TryFrom<&'a [AccountView]> for CloseMarketAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market, histogram, clearing_result, shards @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        if shards.is_empty() {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        verify_signer(authority, true)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;
        verify_writable(clearing_result, true)?;
        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_current_program_account(clearing_result)?;
        for shard in shards {
            verify_writable(shard, true)?;
            verify_current_program_account(shard)?;
        }

        Ok(Self {
            authority,
            market,
            histogram,
            clearing_result,
            shards,
        })
    }
}

impl<'a> InstructionAccounts<'a> for CloseMarketAccounts<'a> {}
