use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{
        verify_current_program, verify_event_authority, verify_signer, verify_system_program,
        verify_writable,
    },
};

/// Accounts for the InitializeMarket instruction.
///
/// # Account Layout
/// 0. `[signer, writable]` payer
/// 1. `[signer]` authority
/// 2. `[signer]` market_seed
/// 3. `[writable]` market - PDA to create
/// 4. `[writable]` histogram - PDA to create
/// 5. `[]` oracle - Pyth PriceUpdateV2 recorded on the market (funding/liquidation)
/// 6. `[]` system_program
/// 7. `[]` event_authority - Event authority PDA
/// 8. `[]` tempo_program - Current program
///
/// Stage A sharding: the OrderSlab shards are created separately by `init_shard`
/// (a market may have too many shards for one tx), so no `order_slab` account here.
pub struct InitializeMarketAccounts<'a> {
    pub payer: &'a AccountView,
    pub authority: &'a AccountView,
    pub market_seed: &'a AccountView,
    pub market: &'a AccountView,
    pub histogram: &'a AccountView,
    pub oracle: &'a AccountView,
    pub system_program: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
}

impl<'a> TryFrom<&'a [AccountView]> for InitializeMarketAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        let [payer, authority, market_seed, market, histogram, oracle, system_program, event_authority, tempo_program] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(payer, true)?;
        verify_signer(authority, false)?;
        verify_signer(market_seed, false)?;

        verify_writable(market, true)?;
        verify_writable(histogram, true)?;

        verify_system_program(system_program)?;

        verify_event_authority(event_authority)?;
        verify_current_program(tempo_program)?;

        Ok(Self {
            payer,
            authority,
            market_seed,
            market,
            histogram,
            oracle,
            system_program,
            event_authority,
            tempo_program,
        })
    }
}

impl<'a> InstructionAccounts<'a> for InitializeMarketAccounts<'a> {}
