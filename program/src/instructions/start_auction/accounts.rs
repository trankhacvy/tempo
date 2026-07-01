use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for the StartAuction instruction (permissionless).
///
/// # Account Layout
/// 0. `[signer]` cranker - permissionless caller
/// 1. `[writable]` market
/// 2. `[writable]` histogram
/// 3. `[]` oracle - the market's bound Pyth `PriceUpdateV2`; the new round's tick
///    window is re-snapped onto it (known-issues §2.7). The processor checks it
///    matches `market.oracle`; a stale/low-confidence price carries the previous
///    window forward (never blocks the roll).
///
/// Stage A sharding: the OrderSlab shards are drained + zeroed by `reset_shard` (one tx
/// per shard) before the roll, so no `order_slab` account here; the roll gates on
/// `market.shards_ready == num_slab_shards`.
pub struct StartAuctionAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub oracle: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for StartAuctionAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, histogram, oracle] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;

        Ok(Self {
            cranker,
            market,
            histogram,
            oracle,
        })
    }
}

impl<'a> InstructionAccounts<'a> for StartAuctionAccounts<'a> {}
