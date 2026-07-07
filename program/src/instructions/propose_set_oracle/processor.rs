use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError, instructions::ProposeSetOracle, state::Market,
    traits::AccountDeserialize,
};

/// Processes ProposeSetOracle (plan.md §3.3): authority-gated staging of a new
/// (oracle, feed id) pair. Requires the market to ALREADY be winding down
/// (`PAUSE_ROLL` set) — users get the full delay window, on a market that is
/// already parking, before a new price regime can take effect.
pub fn process_propose_set_oracle(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProposeSetOracle::try_from((instruction_data, accounts))?;
    let now_slot = Clock::get()?.slot;

    let mut acct = *ix.accounts.market;
    let mut market_data = acct.try_borrow_mut()?;
    {
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;
        if market.paused & Market::PAUSE_ROLL == 0 {
            return Err(TempoProgramError::MarketNotQuiescent.into());
        }
    }
    let market = Market::from_bytes_mut(&mut market_data)?;
    let mut payload = [0u8; 64];
    payload[0..32].copy_from_slice(&ix.data.new_oracle);
    payload[32..64].copy_from_slice(&ix.data.new_feed_id);
    market.stage_pending(
        Market::PENDING_ORACLE,
        &payload,
        now_slot.saturating_add(Market::RISK_UPDATE_DELAY_SLOTS),
    );
    Ok(())
}
