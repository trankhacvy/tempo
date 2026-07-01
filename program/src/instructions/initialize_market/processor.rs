use alloc::vec::Vec;
use pinocchio::{
    account::AccountView,
    cpi::Seed,
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    events::MarketInitializedEvent,
    instructions::InitializeMarket,
    state::{AuctionHistogramHeader, Market, COLLECT_WINDOW_SLOTS},
    traits::{AccountSerialize, AccountSize, EventSerialize, PdaSeeds},
    utils::{create_pda_account, emit_event},
};

/// Processes the InitializeMarket instruction.
///
/// Creates the Market PDA and its empty AuctionHistogram and OrderSlab PDAs,
/// sizing the histogram by `num_ticks` and the slab by `orders_per_auction_cap`.
pub fn process_initialize_market(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitializeMarket::try_from((instruction_data, accounts))?;
    let d = &ix.data;

    // --- Market ---
    let mut market = Market::new(
        d.market_bump,
        *ix.accounts.authority.address(),
        *ix.accounts.market_seed.address(),
        *ix.accounts.oracle.address(),
        d.oracle_feed_id,
        d.tick_size,
        d.num_ticks,
        d.orders_per_auction_cap,
        d.maintenance_margin_bps,
        d.liquidation_penalty_bps,
        d.maker_fee_bps,
        d.taker_fee_bps,
        d.integrator_share_bps,
        d.crank_fee,
        Address::new_from_array(d.collateral_mint),
        d.max_price_move_bps_per_slot,
        d.soft_stale_slots,
        d.initial_margin_bps,
        d.max_position_notional,
        d.num_slab_shards,
    );
    // Open the first collection window: accumulation is blocked until this slot.
    let now_ts = Clock::get()?.unix_timestamp;
    market.set_phase_deadline_slot(Clock::get()?.slot.saturating_add(COLLECT_WINDOW_SLOTS));
    market.validate_pda(ix.accounts.market, program_id, d.market_bump)?;

    // Center the tick window on the oracle if a fresh price is available
    // (known-issues §2.7); otherwise keep the genesis default (floor = tick_size).
    // Best-effort: a market may be provisioned before its Pyth feed is warm, and a
    // wrong/stale oracle here is the authority's own misconfiguration — never block
    // creation on it. `start_auction` re-snaps the window every round thereafter.
    if ix
        .accounts
        .oracle
        .owned_by(&crate::oracle::PYTH_RECEIVER_ID)
    {
        if let Ok(oracle_data) = ix.accounts.oracle.try_borrow() {
            if let Ok(p) = crate::oracle::read_price(
                &oracle_data,
                &d.oracle_feed_id,
                now_ts,
                crate::oracle::MAX_AGE_SECS,
            ) {
                market.recenter_window(p.price_1e8);
            }
        }
    }

    let market_bump = [d.market_bump];
    let market_seeds: Vec<Seed> = market.seeds_with_bump(&market_bump);
    let market_seeds_array: [Seed; 3] = market_seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.payer,
        Market::LEN,
        program_id,
        ix.accounts.market,
        market_seeds_array,
    )?;
    {
        let mut market_account = *ix.accounts.market;
        let mut slice = market_account.try_borrow_mut()?;
        market.write_to_slice(&mut slice)?;
    }

    // --- AuctionHistogram ---
    let market_key = *ix.accounts.market.address();
    let histogram = AuctionHistogramHeader::new(
        d.histogram_bump,
        market_key,
        market.current_auction_id(),
        d.num_ticks,
    );
    histogram.validate_pda(ix.accounts.histogram, program_id, d.histogram_bump)?;

    let histogram_bump = [d.histogram_bump];
    let histogram_seeds: Vec<Seed> = histogram.seeds_with_bump(&histogram_bump);
    let histogram_seeds_array: [Seed; 3] = histogram_seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    let histogram_size = AuctionHistogramHeader::account_size(d.num_ticks);
    create_pda_account(
        ix.accounts.payer,
        histogram_size,
        program_id,
        ix.accounts.histogram,
        histogram_seeds_array,
    )?;
    {
        // Buckets are zero-initialized by CreateAccount; only the header needs writing.
        let mut histogram_account = *ix.accounts.histogram;
        let mut slice = histogram_account.try_borrow_mut()?;
        histogram.write_to_slice(&mut slice)?;
    }

    // Stage A sharding: the OrderSlab shards are NOT created here (a market may have up
    // to `MAX_SLAB_SHARDS`, too many for one tx). They are created one-per-tx by
    // `init_shard` before trading. `Market.num_slab_shards`/`shards_pending` are set by
    // `Market::new` above.

    // Emit event via CPI.
    let event = MarketInitializedEvent {
        market: market_key,
        authority: *ix.accounts.authority.address(),
        tick_size: d.tick_size,
        num_ticks: d.num_ticks,
        orders_per_auction_cap: d.orders_per_auction_cap,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
