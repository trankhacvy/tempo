use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ForceReset instruction (authority-gated escape hatch).
///
/// # Account Layout
/// 0. `[signer]` authority - must match `Market.authority`
/// 1. `[writable]` market
/// 2. `[writable]` histogram
/// 3. `[writable]` order_slab shards (×`num_slab_shards`) — ALL of the market's shards, in one call, so the
///    round is reset atomically (auction id bumped exactly once). At least one is required;
///    the processor rejects the call unless it receives every shard (`shards.len() ==
///    num_slab_shards`), so a partial reset can never leave a stale shard behind.
///    (Account-limit ceiling: a market with more shards than fit in one transaction's account
///    list cannot be force-reset in a single tx — size shard counts accordingly, or recover
///    such a market via repeated `reset_shard` after normal settlement.)
pub struct ForceResetAccounts<'a> {
    pub authority: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub shards: &'a [AccountView],
}

impl<'a> TryFrom<&'a [AccountView]> for ForceResetAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [authority, market, histogram, shards @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        if shards.is_empty() {
            return Err(ProgramError::NotEnoughAccountKeys);
        }

        verify_signer(authority, false)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;
        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        for shard in shards {
            verify_writable(shard, true)?;
            verify_current_program_account(shard)?;
        }

        Ok(Self {
            authority,
            market,
            histogram,
            shards,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ForceResetAccounts<'a> {}
