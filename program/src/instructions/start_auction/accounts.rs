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
/// 3. `[writable]` order_slab
/// 4. `[]` oracle - the market's bound Pyth `PriceUpdateV2`; the new round's tick
///    window is re-snapped onto it (known-issues §2.7). The processor checks it
///    matches `market.oracle`; a stale/low-confidence price carries the previous
///    window forward (never blocks the roll).
pub struct StartAuctionAccounts<'a> {
    pub cranker: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub order_slab: &'a AccountView,
    pub oracle: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for StartAuctionAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [cranker, market, histogram, order_slab, oracle] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(cranker, false)?;
        verify_writable(market, true)?;
        verify_writable(histogram, true)?;
        verify_writable(order_slab, true)?;

        verify_current_program_account(market)?;
        verify_current_program_account(histogram)?;
        verify_current_program_account(order_slab)?;

        Ok(Self {
            cranker,
            market,
            histogram,
            order_slab,
            oracle,
        })
    }
}

impl<'a> InstructionAccounts<'a> for StartAuctionAccounts<'a> {}
