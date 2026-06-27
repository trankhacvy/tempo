use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};
use pinocchio_log::log;

use crate::{
    errors::TempoProgramError,
    events::FundingUpdatedEvent,
    funding::{next_funding_index, period_funding_rate, FUNDING_SCALE},
    instructions::UpdateFunding,
    mark::compute_mark_price,
    oracle::{read_price, DEFAULT_MAX_CONF_BPS, MAX_AGE_SECS, PYTH_RECEIVER_ID},
    state::Market,
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Mark-price band around the oracle (bps).
const MARK_BAND_BPS: u16 = 500;
/// Funding interval (seconds). Uses an hourly interval.
const FUNDING_INTERVAL_SECS: i64 = 3_600;
/// Per-period funding-rate cap (FUNDING_SCALE units). Caps at 1%.
const MAX_FUNDING_RATE: i128 = FUNDING_SCALE / 100;

/// Processes the UpdateFunding instruction (permissionless):
/// reads the bound oracle, derives the mark, accrues the elapsed-time fraction of
/// one funding period into the market's monotonic funding index, and emits a
/// `FundingUpdated` event.
pub fn process_update_funding(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = UpdateFunding::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // Read market: bound oracle, last fills, current funding index + ts, and the
    // effective-price brake state.
    let (oracle_key, feed_id, last_bid, last_ask, funding_index, last_funding_ts) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        (
            market.oracle,
            market.oracle_feed_id,
            market.last_bid_fill_price(),
            market.last_ask_fill_price(),
            market.funding_index(),
            market.last_funding_ts(),
        )
    };

    // The provided oracle must be the one bound to the market + Pyth-owned.
    if ix.accounts.oracle.address() != &oracle_key {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }
    if !ix.accounts.oracle.owned_by(&PYTH_RECEIVER_ID) {
        return Err(TempoProgramError::OracleInvalidAccount.into());
    }

    let clock = Clock::get()?;
    let now_ts = clock.unix_timestamp;
    let now_slot = clock.slot;
    let price = {
        let oracle_data = ix.accounts.oracle.try_borrow()?;
        read_price(&oracle_data, &feed_id, now_ts, MAX_AGE_SECS)?
    };
    // Halt funding when the oracle is too uncertain (system-design §10).
    price.require_confidence(DEFAULT_MAX_CONF_BPS)?;

    let mark = compute_mark_price(last_bid, last_ask, price.price_1e8, MARK_BAND_BPS)?;

    // Elapsed-time fraction of one funding interval, in bps (capped at one period).
    let elapsed = now_ts.saturating_sub(last_funding_ts as i64).max(0);
    let period_fraction_bps =
        (elapsed.saturating_mul(10_000) / FUNDING_INTERVAL_SECS).min(10_000) as u32;

    let rate = period_funding_rate(mark, price.price_1e8, period_fraction_bps, MAX_FUNDING_RATE)?;
    let new_index = next_funding_index(funding_index, rate);

    // Write the advanced index + timestamp back.
    {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_funding_index(new_index);
        market.set_last_funding_ts(now_ts as u64);
        // Brake state + oracle-freshness stamp.
        market.advance_effective_price(price.price_1e8, now_slot);
    }

    log!(
        "tempo: funding index={} mark={} oracle={}",
        new_index,
        mark,
        price.price_1e8
    );

    let event = FundingUpdatedEvent {
        market: market_key,
        funding_index: new_index,
        mark,
        oracle_price_1e8: price.price_1e8,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
