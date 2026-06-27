use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};
use pinocchio_log::log;

use crate::{
    errors::TempoProgramError,
    events::OraclePriceReadEvent,
    instructions::ReadOracle,
    mark::compute_mark_price,
    oracle::{read_price, MAX_AGE_SECS, PYTH_RECEIVER_ID},
    state::Market,
    traits::EventSerialize,
    utils::emit_event,
};

/// Mark-price band around the oracle (bps).
const MARK_BAND_BPS: u16 = 500;

/// Processes the ReadOracle instruction (system-design §10): reads the live
/// Pyth `PriceUpdateV2` account bound to the market, validates owner / feed id /
/// staleness, derives the mark price from the last clearing prices anchored to
/// the oracle, and emits an `OraclePriceRead` event. This is the read-only
/// integration point proven end-to-end against real devnet/mainnet Pyth.
pub fn process_read_oracle(
    program_id: &Address,
    accounts: &[AccountView],
    _instruction_data: &[u8],
) -> ProgramResult {
    let ix = ReadOracle::try_from((_instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- read market: capture the bound oracle + feed id + last fill prices ---
    let (oracle_key, feed_id, last_bid, last_ask) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        (
            market.oracle,
            market.oracle_feed_id,
            market.last_bid_fill_price(),
            market.last_ask_fill_price(),
        )
    };

    // The passed oracle account must be the one bound to the market, and owned
    // by the Pyth receiver program.
    if ix.accounts.oracle.address() != &oracle_key {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }
    if !ix.accounts.oracle.owned_by(&PYTH_RECEIVER_ID) {
        return Err(TempoProgramError::OracleInvalidAccount.into());
    }

    let now_ts = Clock::get()?.unix_timestamp;
    let price = {
        let oracle_data = ix.accounts.oracle.try_borrow()?;
        read_price(&oracle_data, &feed_id, now_ts, MAX_AGE_SECS)?
    };

    // Mark = last bid/ask clearing prices anchored to the oracle within a band
    // (system-design §9.1). With no fills yet, this returns the oracle price.
    let mark = compute_mark_price(last_bid, last_ask, price.price_1e8, MARK_BAND_BPS)?;

    log!("tempo: oracle price_1e8={} mark={}", price.price_1e8, mark);

    let event = OraclePriceReadEvent {
        market: market_key,
        oracle_price_1e8: price.price_1e8,
        exponent: price.exponent,
        publish_time: price.publish_time,
        mark_price: mark,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
