use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    events::OracleRepointedEvent,
    instructions::ApplySetOracle,
    oracle::{read_price, DEFAULT_MAX_CONF_BPS, MAX_AGE_SECS, PYTH_RECEIVER_ID},
    state::{AuctionPhase, Market},
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Processes ApplySetOracle (plan.md §3.3): PERMISSIONLESS after the staged
/// delay, but constrained hard — this is the most dangerous admin power:
/// 1. delay elapsed + kind matches (`take_pending`);
/// 2. still fully paused AND quiescent (round fully settled, all shards reset)
///    — no round may straddle two price regimes;
/// 3. the staged account is LIVE right now: Pyth-owned, parses, matches the
///    staged feed id, fresh, confidence-checked — a proposal for a dead feed
///    can never apply;
/// 4. address + feed id commit atomically (readers check them as a pair). The
///    window re-anchors on the new feed at the next `start_auction`.
pub fn process_apply_set_oracle(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ApplySetOracle::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();
    let clock = Clock::get()?;
    let now_slot = clock.slot;
    let now_ts = clock.unix_timestamp;

    let (old_oracle, new_oracle, new_feed_id) = {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        {
            let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
            // Gate 2: paused + quiescent.
            if market.paused & Market::PAUSE_ROLL == 0 || market.paused & Market::PAUSE_INTAKE == 0
            {
                return Err(TempoProgramError::MarketNotQuiescent.into());
            }
            let phase = market.phase()?;
            let drained = phase == AuctionPhase::Settling || phase == AuctionPhase::Discovered;
            if !drained || market.shards_ready() != market.num_slab_shards() {
                return Err(TempoProgramError::MarketNotQuiescent.into());
            }
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        // Gate 1: delay + kind.
        let payload = market.take_pending(Market::PENDING_ORACLE, now_slot)?;
        let staged_oracle = Address::new_from_array(payload[0..32].try_into().unwrap());
        let staged_feed: [u8; 32] = payload[32..64].try_into().unwrap();

        // Gate 3: the staged account, passed in, must be live and fresh NOW.
        if ix.accounts.new_oracle.address() != &staged_oracle {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        if !ix.accounts.new_oracle.owned_by(&PYTH_RECEIVER_ID) {
            return Err(TempoProgramError::OracleInvalidAccount.into());
        }
        {
            let od = ix.accounts.new_oracle.try_borrow()?;
            let price = read_price(&od, &staged_feed, now_ts, MAX_AGE_SECS)?;
            price.require_confidence(DEFAULT_MAX_CONF_BPS)?;
        }

        // Gate 4: atomic commit.
        let old = market.oracle;
        market.oracle = staged_oracle;
        market.oracle_feed_id = staged_feed;
        market.set_last_good_oracle_slot(now_slot);
        (old, staged_oracle, staged_feed)
    };

    let event = OracleRepointedEvent {
        market: market_key,
        old_oracle,
        new_oracle,
        new_feed_id,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
