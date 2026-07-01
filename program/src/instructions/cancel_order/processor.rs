use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    events::OrderCancelledEvent,
    instructions::CancelOrder,
    state::{
        find_order_by_id_hinted, read_order, write_order, AuctionPhase, Market, Order,
        OrderSlabHeader, OrderStatus,
    },
    traits::{AccountDeserialize, EventSerialize, PdaSeeds},
    utils::emit_event,
};

/// Processes the CancelOrder instruction (Collect phase only).
///
/// Removes a resting order owned by the signing trader, freeing its slot (which
/// decrements the authoritative `OrderSlabHeader.count`). The market is read-only
/// here (PERF-1, known-issues §2.1): `cancel_order` never write-locks `Market`.
pub fn process_cancel_order(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = CancelOrder::try_from((instruction_data, accounts))?;

    // --- phase check ---
    let auction_id = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.require_phase(AuctionPhase::Collect)?;
        market.current_auction_id()
    };

    let market_key = *ix.accounts.market.address();
    let trader = *ix.accounts.trader.address();

    // --- locate + remove order from slab; capture its reservation + whether removing it
    // emptied the shard's unfolded set (resting_count 1→0, so it leaves the aggregate). ---
    let (reserved_margin, shard_became_empty) = {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;

        let capacity = {
            let header = OrderSlabHeader::from_bytes(&slab_data)?;
            if header.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            header.validate_pda(ix.accounts.order_slab, program_id, header.bump)?;
            header.capacity()
        };

        let slot =
            find_order_by_id_hinted(&slab_data, capacity, ix.data.order_id, ix.data.slot_hint)?;
        let order = read_order(&slab_data, capacity, slot)?;

        // must be the owner and still resting (not yet accumulated/consumed)
        if order.trader != trader {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        if order.status != OrderStatus::Resting as u8 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }

        // free the slot
        write_order(&mut slab_data, capacity, slot, &Order::empty())?;

        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        let c = header
            .count()
            .checked_sub(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        header.set_count(c);

        // Stage A completeness: the cancelled order was `Resting` (unfolded — guaranteed by
        // the status check above), so it was counted in this shard's `resting_count`.
        // Decrement it in the SAME borrow as the slot free, so the counter can never drift.
        // Without this, a submit+cancel leaves resting_count > 0 forever and finalize_clear
        // can never satisfy the completeness gate — a permissionless market wedge.
        let new_rc = header
            .resting_count()
            .checked_sub(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        header.set_resting_count(new_rc);

        // `new_rc == 0` ⟺ this shard now holds no unfolded orders, so it leaves the set.
        (order.reserved_margin, new_rc == 0)
    };

    // If cancelling emptied the shard, drop it from the market's completeness aggregate.
    if shard_became_empty {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.set_shards_pending(market.shards_pending().saturating_sub(1));
    }

    // --- release the worst-case margin reserved at submit (missing-features §1.1) ---
    if reserved_margin > 0 {
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        crate::settle_money::release_order_reservation(
            uc_acct,
            program_id,
            &trader,
            reserved_margin,
        )?;
    }

    // Emit event via CPI.
    let event = OrderCancelledEvent {
        market: market_key,
        trader: *ix.accounts.trader.address(),
        order_id: ix.data.order_id,
        auction_id,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
