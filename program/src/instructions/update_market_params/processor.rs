use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    events::MarketParamsUpdatedEvent,
    instructions::UpdateMarketParams,
    state::Market,
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Processes UpdateMarketParams (plan.md §3.2, the HOT set): authority-gated
/// and immediate. Every field is read at use-time from `Market`, so the change
/// applies from the next operation — nothing in flight can strand.
pub fn process_update_market_params(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = UpdateMarketParams::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();
    let d = &ix.data;

    {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        {
            let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
            market.validate_authority(ix.accounts.authority.address())?;
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.maker_fee_bps_le = d.maker_fee_bps.to_le_bytes();
        market.taker_fee_bps_le = d.taker_fee_bps.to_le_bytes();
        market.integrator_share_bps_le = d.integrator_share_bps.to_le_bytes();
        market.crank_fee_le = d.crank_fee.to_le_bytes();
        market.max_price_move_bps_per_slot_le = d.max_price_move_bps_per_slot.to_le_bytes();
        market.set_soft_stale_slots(d.soft_stale_slots);
        market.set_max_position_notional(d.max_position_notional);
        market.set_min_order_notional(d.min_order_notional);
        market.set_max_open_interest(d.max_open_interest);
        market.set_liquidation_reward_floor(d.liquidation_reward_floor);
    }

    let event = MarketParamsUpdatedEvent {
        market: market_key,
        kind: Market::PENDING_NONE, // hot set — applied directly, nothing staged
        payload: [0u8; 64],
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
