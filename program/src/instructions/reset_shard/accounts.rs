use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the ResetShard instruction (Stage A sharding, permissionless).
///
/// # Account Layout
/// 0. `[signer]` cranker - permissionless caller
/// 1. `[writable]` market - increments `shards_ready`
/// 2. `[writable]` order_slab - the drained shard to zero for the next round
pub struct ResetShardAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for ResetShardAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, order_slab] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(order_slab)?;

        Ok(Self {
            cranker,
            market,
            order_slab,
        })
    }
}

impl<'a> InstructionAccounts<'a> for ResetShardAccounts<'a> {}
