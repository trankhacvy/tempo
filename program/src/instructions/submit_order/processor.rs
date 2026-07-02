use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    events::OrderSubmittedEvent,
    instructions::SubmitOrder,
    margin::initial_margin,
    state::{
        find_free_slot, trader_resting_stats, write_order, AuctionPhase, Market, Order, OrderSide,
        OrderSlabHeader, Position, UserCollateral,
    },
    traits::{AccountDeserialize, EventSerialize, PdaAccount, PdaSeeds},
    utils::emit_event,
};

/// Maximum resting orders a single trader may hold in one auction (anti-spam).
/// Bounds how many slab slots one account can occupy, so dust-spam can't
/// lock legitimate traders out with tx-fee-only cost.
const MAX_ORDERS_PER_TRADER: u32 = 8;

/// Processes the SubmitOrder instruction (Collect phase only).
///
/// Validates phase + price/quantity, **reserves the order's worst-case initial
/// margin** into the trader's collateral ledger (missing-features §1.1) on a
/// money-path market, and inserts the order into a free slab slot. The market is
/// read-only here (PERF-1, known-issues §2.1): the authoritative live-order count
/// is `OrderSlabHeader.count`, so `submit_order` never write-locks `Market`.
///
/// # Pre-trade margin reservation (missing-features §1.1)
/// A batch auction discovers the clearing price *after* matching, so the only way
/// to guarantee a matched trade can settle is to reserve, at submit, an upper bound
/// on the margin its fill could ever require. A buy clears at ≤ its limit price; a
/// sell clears at ≤ the histogram window top; so reserving `worst_qty · worst_price
/// · initial_bps` and locking it now means `settle_fill` only ever *releases* — it
/// can never revert for lack of collateral (which would wedge the round). A
/// `reduce_only` order reserves only the portion that would open new exposure, so a
/// close is never blocked.
pub fn process_submit_order(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = SubmitOrder::try_from((instruction_data, accounts))?;
    let trader = *ix.accounts.trader.address();
    let market_key = *ix.accounts.market.address();

    // --- validate market (read), capture fields we need ---
    let (
        capacity,
        auction_id,
        maintenance_bps,
        initial_bps,
        window_top_price,
        max_position_notional,
        num_slab_shards,
    ) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Collect)?;
        market.validate_price(ix.data.price)?;
        // ensure the price falls in the histogram window
        market.price_to_tick(ix.data.price)?;
        // The highest in-window price (top tick) — a sell can clear no higher, so it
        // bounds the sell-side worst-case fill. Reuses the single tick→price mapping
        // (no open-coded copy that could drift from the oracle-anchored window, §2.7).
        let window_top_price = market.tick_to_price(market.num_ticks().saturating_sub(1))?;
        (
            market.orders_per_auction_cap(),
            market.current_auction_id(),
            market.maintenance_margin_bps(),
            market.initial_margin_bps(),
            window_top_price,
            market.max_position_notional(),
            market.num_slab_shards(),
        )
    };

    // The requested shard must be within the market's shard set (Stage A sharding).
    if ix.data.shard_id >= num_slab_shards {
        return Err(TempoProgramError::ShardOutOfRange.into());
    }

    // Reject an already-expired order (DDR-3 Correction-2 item 4): an order whose
    // `expires_at_auction` is already reached at submit time can never fold or fill
    // this round or any later one, so it would only rest as dead margin the reaper
    // must collect. `0` means GTC (never expires). Uses the same `<=` boundary as
    // `settle_fill`'s consume-after-fill check (the permissionless reaper uses strict
    // `<`, but an order submitted AT its expiry auction is never entitled to fold).
    if ix.data.expires_at_auction != 0 && ix.data.expires_at_auction <= auction_id {
        return Err(TempoProgramError::OrderAlreadyExpired.into());
    }

    // --- validate order slab PDA matches this market + anti-spam + reduce headroom ---
    let already_same_side = {
        let slab_data = ix.accounts.order_slab.try_borrow()?;
        let slab = OrderSlabHeader::from_bytes(&slab_data)?;
        if slab.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        // The supplied slab account is the canonical PDA for its own stored shard_id
        // (validate_pda ties the address to the seeds, which include shard_id); require
        // that stored shard matches the requested one so data and account agree.
        if slab.shard_id() != ix.data.shard_id {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
        if slab.capacity() != capacity {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        // One slab pass: the trader's resting-order count (anti-spam cap) and their
        // same-side resting quantity (reduce-only headroom, charged below so resting
        // reduces can't collectively flip the position without reserving margin).
        let (resting_count, same_side_qty) = trader_resting_stats(
            &slab_data,
            capacity,
            &trader,
            ix.data.side,
            ix.data.reduce_only,
        )?;
        // NOTE (Stage B): `MAX_ORDERS_PER_TRADER` is enforced PER SHARD — `trader_resting_stats`
        // scans only the requested shard. With resting orders a trader's standing orders can
        // span shards, so this is a per-shard standing cap, not a global one. That is acceptable
        // when the client routes a trader to a consistent shard (e.g. `hash(trader) % shards`);
        // a truly global cap would need an all-shard scan (a follow-up), which would re-serialize
        // submit on every shard and is deliberately avoided (Design Z / DDR-1).
        if resting_count >= MAX_ORDERS_PER_TRADER {
            return Err(TempoProgramError::TraderOrderCapReached.into());
        }
        same_side_qty
    };

    // Worst-case execution price for this order, snapshotted now (Stage B, §3.3): a buy
    // clears at ≤ its limit price; a sell at ≤ the window top. Persisted on the order so a
    // resting order re-margins against this FIXED price every round — its collateral
    // requirement can't drift as the oracle-anchored window moves. Computed for every market
    // (not just money-path) so the stored snapshot is always meaningful; on a no-money-path
    // market it is simply never used (reserved_margin stays 0).
    let is_buy = OrderSide::from_u8(ix.data.side)? == OrderSide::Buy;
    let worst_price = if is_buy {
        ix.data.price
    } else {
        window_top_price
    };

    // --- reserve worst-case initial margin (money-path markets only) ---
    let reserved_margin: u64 = if maintenance_bps == 0 {
        // No-money-path (clearing benchmark) market: nothing to reserve, and the
        // optional position/collateral accounts are not required.
        0
    } else {
        let pos_acct = ix
            .accounts
            .position
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;

        // The trader's current signed position size (validate it is theirs).
        let pos_size: i128 = {
            let pos_data = pos_acct.try_borrow()?;
            let position = Position::from_bytes(&pos_data)?;
            if position.owner != trader || position.market != market_key {
                return Err(TempoProgramError::InvalidOrderOwner.into());
            }
            position.validate_self(pos_acct, program_id)?;
            position.size() as i128
        };

        // Reduce-only reserves the FULL worst-case initial margin (DDR-3 Correction-2 item 3):
        // NO discount. Reduce-only cannot be perfectly honored in a batch auction — the fill
        // quantity is fixed at fold but the position at settle can move (liquidate/funding/other
        // fills), so the order may open against intent. Clamping the fill at settle to enforce
        // shrink-only breaks conservation (Correction #1), so the only safe path is to let it
        // open a COLLATERALIZED position. Hence a reduce-only order reserves the same full
        // worst-case margin as any normal order; the persisted `reduce_only` byte's sole
        // remaining job is to force `Consumed` at settle (never re-arm `Resting`).
        let abs_pos = pos_size.unsigned_abs();
        let qty = ix.data.quantity as u128;
        // `already_same_side` (reduce headroom anti-spam accounting) is still scanned above so
        // resting reduces can't collectively flip the position, but it no longer discounts the
        // reserved margin.
        let _ = already_same_side;
        let opening_qty: u64 = ix.data.quantity;

        // Per-position notional cap (missing-features §1.2): bound the order's
        // worst-case NEW exposure only. A same-side order grows `|pos|` by `qty`; an
        // opposite-side order first *reduces* it, so only the flip remainder past
        // `|pos|` is new. A pure reduce/close adds zero new exposure and is therefore
        // never blocked — a trader can always de-risk even after the oracle-anchored
        // window rose past the cap (the cap is a risk-INCREASE gate, not a hold gate).
        if max_position_notional > 0 {
            let same_side = (pos_size >= 0) == is_buy;
            let new_exposure_abs = if same_side {
                abs_pos.saturating_add(qty)
            } else {
                qty.saturating_sub(abs_pos)
            };
            if new_exposure_abs.saturating_mul(worst_price as u128) > max_position_notional {
                return Err(TempoProgramError::PositionLimitExceeded.into());
            }
        }

        let reserve = initial_margin(opening_qty, worst_price, initial_bps);

        // Lock it now. `lock` debits free balance and fails with
        // `InsufficientCollateral` if the trader can't back the worst case — a clean
        // pre-trade rejection instead of a stuck settlement.
        {
            let mut uc = *uc_acct;
            let mut uc_data = uc.try_borrow_mut()?;
            let user_collateral = UserCollateral::from_bytes_mut(&mut uc_data)?;
            if user_collateral.owner != trader {
                return Err(TempoProgramError::InvalidOrderOwner.into());
            }
            user_collateral.validate_self(uc_acct, program_id)?;
            user_collateral.lock(reserve)?;
        }
        reserve
    };

    // --- insert into a free slot ---
    let order_id;
    let order_slot;
    {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;

        // read header fields
        let (capacity, next_order_id, count, hint) = {
            let header = OrderSlabHeader::from_bytes(&slab_data)?;
            (
                header.capacity(),
                header.next_order_id(),
                header.count(),
                header.next_free_hint(),
            )
        };
        if count >= capacity {
            return Err(TempoProgramError::OrderSlabFull.into());
        }

        // Allocate from the forward cursor (O(1) on the common forward-fill path);
        // `find_free_slot` wraps to reclaim holes if the tail is full.
        let slot = find_free_slot(&slab_data, capacity, hint)?;
        order_id = next_order_id;
        order_slot = slot;

        let side = OrderSide::from_u8(ix.data.side)?;
        // Taker-only (§1.3): submit_order never produces maker flow.
        let mut order = Order::new_resting(order_id, trader, side, ix.data.price, ix.data.quantity);
        // Carry the worst-case reservation on the order so cancel/settle release
        // exactly this amount (missing-features §1.1).
        order.reserved_margin = reserved_margin;
        // Stage B resting fields: the fixed worst-case price snapshot (for stable
        // re-margining across rounds, §3.3) and the client-chosen expiry (0 = GTC).
        order.worst_price = worst_price;
        order.expires_at_auction = ix.data.expires_at_auction;
        // Persist reduce_only (DDR-3 correction #1): its ONLY job now is to force
        // `settle_fill` to mark the order `Consumed` (never re-armed `Resting`), so a
        // reduce-only order can never carry across rounds and open new (under-reserved)
        // exposure the market gapped it into. It applies its full computed fill this
        // round with no settle-time clamp.
        order.reduce_only = u8::from(ix.data.reduce_only);
        write_order(&mut slab_data, capacity, slot, &order)?;

        // bump header counters + advance the allocation cursor past this slot
        // (saturating at capacity; the next submit wraps to reclaim any holes).
        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        header.set_next_order_id(
            next_order_id
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        header.set_count(
            count
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        // Design Z (DDR-1): the fresh order is `Resting` (not yet folded). We track it in
        // this shard's own `resting_count` — shard-local, maintained in the same borrow as
        // the order write, so it can never drift — purely so the keeper can skip empty shards.
        // Completeness itself is proven authoritatively by `finalize_clear` scanning every
        // shard, so `submit_order` writes NO shared account (`Market` stays read-only here) and
        // submits into different shards run fully in parallel.
        let new_rc = header
            .resting_count()
            .checked_add(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        header.set_resting_count(new_rc);
        header.set_next_free_hint(slot.saturating_add(1).min(capacity));
    }

    // Emit event via CPI (carries the order id + fields; no log!, §1).
    let event = OrderSubmittedEvent {
        market: market_key,
        trader,
        order_id,
        auction_id,
        price: ix.data.price,
        quantity: ix.data.quantity,
        slot: order_slot,
        side: ix.data.side,
        // Taker-only (§1.3); kept in the event for indexer schema stability.
        is_maker: 0,
        shard_id: ix.data.shard_id,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
