use alloc::vec::Vec;
use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    events::OrderCancelledEvent,
    instructions::CancelAllOrders,
    state::{read_order, write_order, Market, Order, OrderSlabHeader, OrderStatus},
    traits::{AccountDeserialize, EventSerialize, PdaSeeds},
    utils::emit_event,
};

/// Processes the CancelAllOrders instruction (missing-features §2.7): cancel
/// EVERY still-`Resting` order in one shard owned by the signer, in one
/// transaction — the "pull my whole book" panic button a market maker needs
/// when it must flatten fast.
///
/// Semantics are `cancel_order` applied per matching slot, with three deliberate
/// differences:
///  * **Owner path only** — no reaper branch. Strangers' orders (expired or
///    not) are never touched; permissionless reaping of an expired order stays
///    on `cancel_order`, keeping the strict-`<` reap boundary in exactly one
///    place.
///  * **One summed release** — the freed worst-case reservations are released
///    to the owner's ledger as a single `Σ reserved_margin` credit (one
///    borrow, one write) instead of per-order.
///  * **Zero matches is a success** — a no-op batch cancel is not an error, so
///    a client can fire-and-forget it per shard without tracking which shards
///    hold orders.
///
/// Like `cancel_order` it is accepted in ANY phase (always-open, DDR-4): only
/// still-`Resting` orders — never folded into the histogram — are removed, so
/// the histogram/clearing state can never desync from the slab. Scan cost is
/// bounded by the shard capacity (≤ 90 slots), with ≤ 8 matches under the
/// per-trader-per-shard anti-spam cap.
pub fn process_cancel_all_orders(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = CancelAllOrders::try_from((instruction_data, accounts))?;

    // Phase byte still validated; id captured for the per-order events.
    let auction_id = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        let _ = market.phase()?;
        market.current_auction_id()
    };

    let market_key = *ix.accounts.market.address();
    let signer = *ix.accounts.trader.address();

    // --- scan the shard: free every Resting slot owned by the signer ---
    let (total_release, cancelled_ids) = {
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

        let mut total_release = 0u64;
        let mut cancelled_ids: Vec<u64> = Vec::new();
        for slot in 0..capacity {
            let order = read_order(&slab_data, capacity, slot)?;
            if order.status != OrderStatus::Resting as u8 {
                continue;
            }
            if order.trader != signer {
                continue;
            }
            total_release = total_release
                .checked_add(order.reserved_margin)
                .ok_or(TempoProgramError::MathOverflow)?;
            write_order(&mut slab_data, capacity, slot, &Order::empty())?;
            cancelled_ids.push(order.order_id);
        }

        // Header counters in the SAME borrow as the slot frees (never drifts) —
        // every cancelled order was `Resting`, so both `count` and the
        // shard-local `resting_count` drop by the batch size.
        if !cancelled_ids.is_empty() {
            let n = cancelled_ids.len() as u32;
            let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
            let c = header
                .count()
                .checked_sub(n)
                .ok_or(TempoProgramError::MathOverflow)?;
            header.set_count(c);
            let rc = header
                .resting_count()
                .checked_sub(n)
                .ok_or(TempoProgramError::MathOverflow)?;
            header.set_resting_count(rc);
        }

        (total_release, cancelled_ids)
    };

    // --- one summed release of the worst-case margin (missing-features §1.1) ---
    // The signer IS the owner here (owner-path only), and
    // `release_order_reservation` re-validates the ledger belongs to them.
    if total_release > 0 {
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        crate::settle_money::release_order_reservation(
            uc_acct,
            program_id,
            &signer,
            total_release,
        )?;
    }

    // One OrderCancelled event per cancelled order — indexers already decode
    // this event, so a batch cancel needs no new event type.
    for order_id in cancelled_ids {
        let event = OrderCancelledEvent {
            market: market_key,
            trader: signer,
            order_id,
            auction_id,
        };
        emit_event(
            program_id,
            ix.accounts.event_authority,
            ix.accounts.tempo_program,
            &event.to_bytes(),
        )?;
    }

    Ok(())
}
