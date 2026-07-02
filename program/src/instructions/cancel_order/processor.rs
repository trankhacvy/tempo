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
/// Removes a resting order, freeing its slot (which decrements the authoritative
/// `OrderSlabHeader.count`). The market is read-only here (PERF-1, known-issues
/// §2.1): `cancel_order` never write-locks `Market`.
///
/// Authorization (DDR-3 correction #2 + Correction-2 item 4): the order's **owner**
/// may always cancel; **anyone** may reap an order only *after its last active round*
/// (strict `expires_at_auction < current_auction_id`). A passive resting order is never folded, so `settle_fill`
/// (the only other place expiry is handled) never runs on it — without a
/// permissionless reaper its `reserved_margin` would stay locked forever if the
/// window never returns. In BOTH paths the released margin returns to the
/// **owner's** ledger (`order.trader`), never the signer — a reaper cannot redirect
/// margin to itself.
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
    let signer = *ix.accounts.trader.address();

    // --- locate + remove order from slab; capture its reservation + owner ---
    let (reserved_margin, order_owner) = {
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

        // must still be resting (not yet accumulated/consumed)
        if order.status != OrderStatus::Resting as u8 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }

        // DDR-3 correction #2 + Correction-2 item 4: the owner may always cancel; anyone
        // may reap an EXPIRED order (permissionless GC of a passive order's locked margin).
        // The permissionless reap boundary is STRICT `<` — an order is reapable by a
        // non-owner only AFTER its last active round. Using `<=` would let anyone strip an
        // order during the very round it is still entitled to fold and fill (a denial-of-fill
        // grief). `settle_fill` keeps its own `<=` consume-after-fill boundary UNCHANGED. The
        // reaper never benefits — the reservation always returns to `order.trader` below.
        let reapable = order.expires_at_auction != 0 && order.expires_at_auction < auction_id;
        if !reapable && order.trader != signer {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }

        // free the slot
        write_order(&mut slab_data, capacity, slot, &Order::empty())?;

        let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
        let c = header
            .count()
            .checked_sub(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        header.set_count(c);

        // Design Z (DDR-1): the cancelled order was `Resting` (unfolded — guaranteed by the
        // status check above), so it was counted in this shard's own `resting_count`. Decrement
        // it in the SAME borrow as the slot free so it can never drift. This is shard-local only
        // (the keeper uses it to skip empty shards); completeness is proven by finalize scanning
        // every shard, so `cancel_order` writes NO shared account (`Market` stays read-only).
        let new_rc = header
            .resting_count()
            .checked_sub(1)
            .ok_or(TempoProgramError::MathOverflow)?;
        header.set_resting_count(new_rc);

        (order.reserved_margin, order.trader)
    };

    // --- release the worst-case margin reserved at submit (missing-features §1.1) ---
    //
    // Always release to the ORDER OWNER (`order_owner`), never the signer — a reaper
    // of an expired order must not be able to redirect margin to itself
    // (DDR-3 correction #2). `release_order_reservation` also validates the passed
    // `user_collateral` belongs to `order_owner`.
    if reserved_margin > 0 {
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        crate::settle_money::release_order_reservation(
            uc_acct,
            program_id,
            &order_owner,
            reserved_margin,
        )?;
    }

    // Emit event via CPI. The `trader` is the order's OWNER, not the (possibly
    // reaping) signer.
    let event = OrderCancelledEvent {
        market: market_key,
        trader: order_owner,
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
