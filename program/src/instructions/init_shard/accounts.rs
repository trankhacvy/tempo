use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program_account, verify_signer, verify_system_program, verify_writable,
    },
};

/// Accounts for the InitShard instruction (Stage A sharding).
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[]` market
/// 2. `[writable]` order_slab - the shard PDA `[b"order_slab", market, shard_id]` to create
/// 3. `[]` system_program
pub struct InitShardAccounts<'a> {
    pub payer: &'a AccountView,
    pub market: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub system_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitShardAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, market, order_slab, system_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_system_program(system_program)?;

        Ok(Self {
            payer,
            market,
            order_slab,
            system_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitShardAccounts<'a> {}
