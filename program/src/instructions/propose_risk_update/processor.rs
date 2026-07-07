use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    instructions::{initialize_market::validate_risk_config, ProposeRiskUpdate},
    state::Market,
    traits::AccountDeserialize,
};

/// Processes ProposeRiskUpdate (plan.md §3.2): authority-gated staging of the
/// risk-class params. Bounds are RE-validated here with the same shared
/// function `initialize_market` uses, so an out-of-range config can never even
/// be staged. Applying is permissionless after `RISK_UPDATE_DELAY_SLOTS`.
pub fn process_propose_risk_update(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProposeRiskUpdate::try_from((instruction_data, accounts))?;
    let d = &ix.data;

    validate_risk_config(
        d.maintenance_margin_bps,
        d.initial_margin_bps,
        d.liquidation_penalty_bps,
        d.liquidation_close_buffer_bps,
    )?;

    let now_slot = Clock::get()?.slot;
    let mut acct = *ix.accounts.market;
    let mut market_data = acct.try_borrow_mut()?;
    {
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;
    }
    let market = Market::from_bytes_mut(&mut market_data)?;
    let mut payload = [0u8; 8];
    payload[0..2].copy_from_slice(&d.maintenance_margin_bps.to_le_bytes());
    payload[2..4].copy_from_slice(&d.initial_margin_bps.to_le_bytes());
    payload[4..6].copy_from_slice(&d.liquidation_penalty_bps.to_le_bytes());
    payload[6..8].copy_from_slice(&d.liquidation_close_buffer_bps.to_le_bytes());
    market.stage_pending(
        Market::PENDING_RISK_PARAMS,
        &payload,
        now_slot.saturating_add(Market::RISK_UPDATE_DELAY_SLOTS),
    );
    Ok(())
}
