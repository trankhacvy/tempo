use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::{reset_round_to_collect, StartAuction},
    oracle::{read_price, MAX_AGE_SECS, PYTH_RECEIVER_ID},
    state::{AuctionPhase, Market},
    traits::AccountDeserialize,
};

/// Processes the StartAuction instruction — rolls the market into its next
/// round (system-design §7, freeze model).
///
/// Permissionless: only succeeds once the prior round is fully settled
/// (`Settling` phase with an empty order slab), so there is nothing to grief.
/// Bumps the auction id, zeroes the histogram buckets and slab slots, resets
/// the counters, and reopens the `Collect` phase.
///
/// # Freeze model (system-design §7)
/// A new round cannot open until the prior one is fully settled — no
/// pipelining. The liveness failure mode is *delay*, not loss: any honest party
/// can keep cranking `process_chunk`/`finalize_clear`/`settle_fill` to drain the
/// current round. A supervised abort path for a wedged round is a future
/// addition (an admin-gated force-reset).
pub fn process_start_auction(
    program_id: &Address,
    accounts: &[AccountView],
    _instruction_data: &[u8],
) -> ProgramResult {
    let ix = StartAuction::try_from((_instruction_data, accounts))?;

    // --- validate phase + capture params ---
    let (num_ticks, auction_id, oracle_key, feed_id, num_slab_shards, shards_ready) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        // Roll from Settling (the normal path) or from Discovered when the book was
        // empty: an order-less round never reaches Settling (no settle_fill to make
        // the transition), so without this it wedges in Discovered forever. The
        // shards-ready precondition below is the real gate — a round whose shards are
        // not all drained + reset is refused.
        let phase = market.phase()?;
        if phase != AuctionPhase::Settling && phase != AuctionPhase::Discovered {
            return Err(TempoProgramError::AuctionWrongPhase.into());
        }
        (
            market.num_ticks(),
            market.current_auction_id(),
            market.oracle,
            market.oracle_feed_id,
            market.num_slab_shards(),
            market.shards_ready(),
        )
    };

    // --- precondition: every shard must be drained + reset for the next round ---
    // Stage A sharding: `reset_shard` (one tx per shard) drains and re-arms each shard
    // and increments `shards_ready`. The freeze model holds until they are ALL ready, so
    // a new round cannot open on a partially-settled book (clearing-protocol §4).
    if shards_ready != num_slab_shards {
        return Err(TempoProgramError::AuctionNotComplete.into());
    }

    // The supplied oracle MUST be the market's bound feed — a hostile cranker must
    // not be able to mis-center the new window with a fake account. A wrong/absent
    // oracle is a malformed call (hard error); a *stale* price on the right account
    // is handled below by carrying the previous window forward.
    if *ix.accounts.oracle.address() != oracle_key {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }

    let next_id = auction_id
        .checked_add(1)
        .ok_or(TempoProgramError::MathOverflow)?;
    let now = Clock::get()?;
    let slot = now.slot;
    let now_ts = now.unix_timestamp;

    reset_round_to_collect(
        program_id,
        ix.accounts.market,
        ix.accounts.histogram,
        num_ticks,
        next_id,
        slot,
    )?;

    // Re-snap the new round's tick window onto the oracle (known-issues §2.7).
    // Best-effort by design: a fresh, confidence-checked price recenters; a stale
    // or low-confidence one is skipped (`read_price` errors) and `recenter_window`
    // is never called, so the previous floor carries forward and the roll still
    // succeeds — a degraded feed delays recentering, it never halts the market.
    // This runs only once per round (here), so the window is frozen for the whole
    // round, keeping the histogram's tick→price meaning constant.
    if ix.accounts.oracle.owned_by(&PYTH_RECEIVER_ID) {
        let fresh_price = {
            let oracle_data = ix.accounts.oracle.try_borrow()?;
            read_price(&oracle_data, &feed_id, now_ts, MAX_AGE_SECS)
                .ok()
                .map(|p| p.price_1e8)
        };
        if let Some(px) = fresh_price {
            let mut market_account = *ix.accounts.market;
            let mut market_data = market_account.try_borrow_mut()?;
            let market = Market::from_bytes_mut(&mut market_data)?;
            market.recenter_window(px);
        }
    }

    Ok(())
}
