use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    events::MarketParamsUpdatedEvent,
    instructions::ApplyRiskUpdate,
    state::Market,
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Processes ApplyRiskUpdate (plan.md §3.2): PERMISSIONLESS — the delay is
/// enforced by consensus (`take_pending` checks the effective slot), so anyone
/// can complete a staged risk change; the authority's honesty is not part of
/// the safety argument (crank philosophy).
pub fn process_apply_risk_update(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ApplyRiskUpdate::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();
    let now_slot = Clock::get()?.slot;

    let payload = {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        {
            Market::from_account(&market_data, ix.accounts.market, program_id)?;
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        let payload = market.take_pending(Market::PENDING_RISK_PARAMS, now_slot)?;
        market.maintenance_margin_bps_le = payload[0..2].try_into().unwrap();
        market.initial_margin_bps_le = payload[2..4].try_into().unwrap();
        market.liquidation_penalty_bps_le = payload[4..6].try_into().unwrap();
        market.liquidation_close_buffer_bps_le = payload[6..8].try_into().unwrap();
        payload
    };

    let event = MarketParamsUpdatedEvent {
        market: market_key,
        kind: Market::PENDING_RISK_PARAMS,
        payload,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
