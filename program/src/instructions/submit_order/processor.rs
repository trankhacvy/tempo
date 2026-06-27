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
/// money-path market, inserts the order into a free slab slot, and bumps the
/// market's active-order count.
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
        )
    };

    // --- validate order slab PDA matches this market + anti-spam + reduce headroom ---
    let already_same_side = {
        let slab_data = ix.accounts.order_slab.try_borrow()?;
        let slab = OrderSlabHeader::from_bytes(&slab_data)?;
        if slab.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        slab.validate_pda(ix.accounts.order_slab, program_id, slab.bump)?;
        if slab.capacity() != capacity {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        // One slab pass: the trader's resting-order count (anti-spam cap) and their
        // same-side resting quantity (reduce-only headroom, charged below so resting
        // reduces can't collectively flip the position without reserving margin).
        let (resting_count, same_side_qty) =
            trader_resting_stats(&slab_data, capacity, &trader, ix.data.side)?;
        if resting_count >= MAX_ORDERS_PER_TRADER {
            return Err(TempoProgramError::TraderOrderCapReached.into());
        }
        same_side_qty
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

        let is_buy = OrderSide::from_u8(ix.data.side)? == OrderSide::Buy;
        // Worst-case execution price for this order: a buy clears at ≤ its limit; a
        // sell clears at ≤ the window top.
        let worst_price = if is_buy {
            ix.data.price
        } else {
            window_top_price
        };

        // Reduce-aware opening quantity: the portion of this order that would OPEN
        // new exposure. A reduce-only order against an opposite position reserves
        // only what flips past the (already-claimed) reduce headroom.
        let abs_pos = pos_size.unsigned_abs();
        let qty = ix.data.quantity as u128;
        let is_reducing = (pos_size > 0 && !is_buy) || (pos_size < 0 && is_buy);
        let opening_qty: u64 = if ix.data.reduce_only && is_reducing {
            let headroom = abs_pos.saturating_sub(already_same_side as u128);
            u64::try_from(qty.saturating_sub(headroom)).unwrap_or(ix.data.quantity)
        } else {
            ix.data.quantity
        };

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
        header.set_next_free_hint(slot.saturating_add(1).min(capacity));
    }

    // --- bump market active order count ---
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        let v = market
            .active_order_count()
            .checked_add(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        market.set_active_order_count(v);
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
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
