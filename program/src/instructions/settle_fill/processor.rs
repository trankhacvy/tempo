use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    clearing::{fill_against_cross, AuctionCross},
    errors::TempoProgramError,
    events::FillSettledEvent,
    instructions::SettleFill,
    margin::{initial_margin, signed_protocol_fee},
    state::{
        find_order_by_id_hinted, read_order, write_order, AuctionPhase, ClearingResult, Market,
        OrderSide, OrderSlabHeader, OrderStatus, Position, UserCollateral, Vault,
    },
    traits::{AccountDeserialize, EventSerialize, PdaAccount, PdaSeeds},
    utils::emit_event,
};

/// Processes the SettleFill instruction — Phase 3 SETTLE
/// (clearing-protocol §3). Permissionless to trigger. Computes ONE order's
/// fill from the published `ClearingResult` and marks it consumed.
///
/// Fills are *pulled, not pushed*: each call settles exactly one order, so the
/// per-position write cost is paid in that order's own transaction
/// (system-design §1, clearing-protocol §3).
///
/// The order's auction (bid vs ask) is chosen from its side — slab orders are
/// taker-only (§1.3), so a taker sell settles in the bid auction and a taker buy
/// in the ask auction — and the matching side of the `ClearingResult` supplies
/// the marginal-tick rationing constants.
///
/// A non-zero fill is always recorded: the `position` account is mandatory
/// whenever `fill > 0` (the matched trade is applied to it — VWAP entry / realized
/// PnL — so it can never be silently discarded). When `user_collateral` is also
/// supplied, funding is settled, realized PnL is flushed and margin is re-locked
/// to the new size, and the `vault` is required if a protocol fee applies. A
/// zero-fill order (it matched nothing) is simply consumed.
pub fn process_settle_fill(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = SettleFill::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- validate market phase; capture params ---
    let (
        tick_size,
        num_ticks,
        window_floor,
        auction_id,
        funding_index,
        maintenance_bps,
        initial_bps,
        taker_fee_bps,
        integrator_share_bps,
        market_collateral_mint,
        social_long,
        social_short,
    ) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        let phase = market.phase()?;
        if phase != AuctionPhase::Discovered && phase != AuctionPhase::Settling {
            return Err(TempoProgramError::AuctionWrongPhase.into());
        }
        (
            market.tick_size(),
            market.num_ticks(),
            market.window_floor_price(),
            market.current_auction_id(),
            market.funding_index(),
            market.maintenance_margin_bps(),
            market.initial_margin_bps(),
            market.taker_fee_bps(),
            market.integrator_share_bps(),
            market.collateral_mint,
            market.social_loss_index_long(),
            market.social_loss_index_short(),
        )
    };

    // Transition Discovered -> Settling on first settle.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        if market.phase == AuctionPhase::Discovered as u8 {
            market.phase = AuctionPhase::Settling as u8;
        }
    }

    // --- read + validate clearing result ---
    let cr = {
        let cr_data = ix.accounts.clearing_result.try_borrow()?;
        let cr = ClearingResult::from_bytes(&cr_data)?;
        if cr.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        cr.validate_self(ix.accounts.clearing_result, program_id)?;
        if cr.auction_id() != auction_id {
            return Err(TempoProgramError::AuctionIdMismatch.into());
        }
        cr.clone()
    };

    // --- locate the order + compute its fill ---
    let fill;
    let order_trader;
    let order_side;
    let settle_price;
    // The margin to release back to free balance in this settle (Stage B): the FULL
    // reservation when the order leaves the book (fully filled / expired), or only the
    // filled slice when it re-arms as a resting order (the leftover keeps its margin
    // locked). `0` when there is nothing to release.
    let release_amount;
    let order_shard_id;
    {
        let mut slab_account = *ix.accounts.order_slab;
        let mut slab_data = slab_account.try_borrow_mut()?;

        let (capacity, shard_id) = {
            let header = OrderSlabHeader::from_bytes(&slab_data)?;
            if header.market != market_key {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            // Stage A: `validate_pda` ties the account to its stored shard_id (a PDA seed),
            // so this confirms the passed account is a canonical shard of this market.
            header.validate_pda(ix.accounts.order_slab, program_id, header.bump)?;
            (header.capacity(), header.shard_id())
        };
        order_shard_id = shard_id;

        let slot =
            find_order_by_id_hinted(&slab_data, capacity, ix.data.order_id, ix.data.slot_hint)?;
        let order = read_order(&slab_data, capacity, slot)?;

        // Order must have been accumulated (folded) and not yet consumed.
        if order.status != OrderStatus::Accumulated as u8 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }

        let order_tick =
            crate::state::price_to_tick_raw(order.price, window_floor, tick_size, num_ticks)?;

        // Pick the auction this order belongs to (system-design §1). Slab orders
        // are taker-only (§1.3): a taker sell clears in the bid auction, a taker
        // buy in the ask auction.
        let side = OrderSide::from_u8(order.side)?;
        let is_bid_auction = side == OrderSide::Sell;
        let (cross, auction_price) = if is_bid_auction {
            (
                AuctionCross {
                    marginal_tick: cr.bid_marginal_tick(),
                    matched_volume: cr.bid_matched_volume(),
                    volume_allocated_to_marginal_tick: cr.bid_volume_allocated_to_marginal_tick(),
                    total_qty_at_marginal_tick: cr.bid_total_qty_at_marginal_tick(),
                    rationed_side: cr.bid_rationed_side,
                },
                cr.bid_clearing_price(),
            )
        } else {
            (
                AuctionCross {
                    marginal_tick: cr.ask_marginal_tick(),
                    matched_volume: cr.ask_matched_volume(),
                    volume_allocated_to_marginal_tick: cr.ask_volume_allocated_to_marginal_tick(),
                    total_qty_at_marginal_tick: cr.ask_total_qty_at_marginal_tick(),
                    rationed_side: cr.ask_rationed_side,
                },
                cr.ask_clearing_price(),
            )
        };

        // One shared fill classifier (known-issues §3) — the SAME marginal-tick
        // boundary `settle_maker_quote` uses, so maker and taker fills can never
        // drift and stop netting to the matched volume. A buy is the demand side,
        // a sell the supply side; the rationed marginal tick uses this order's
        // fold-time `cum_before` snapshot (process_chunk, §2.7) so its telescoping
        // slice makes the rationed side sum to exactly `vol_alloc`. The conserving
        // qty is `remaining` (== the folded quantity), not a re-scan of the slab.
        fill = fill_against_cross(
            &cross,
            side == OrderSide::Buy,
            order_tick,
            order.remaining,
            order.cum_before,
        )?;

        order_trader = order.trader;
        order_side = order.side;
        settle_price = auction_price;

        let new_remaining = order
            .remaining
            .checked_sub(fill)
            .ok_or(TempoProgramError::MathOverflow)?;

        // Stage B resting orders (plan §3.2): decide whether this order LEAVES the book or
        // carries to the next round.
        //   - fully filled (`fill == remaining`) or expired → `Consumed`; it leaves the
        //     book (`count -= 1`) and its full leftover reservation is released.
        //   - partial or zero fill AND still live → re-armed as `Resting`: `remaining`
        //     shrinks by the fill, `cum_before` is cleared for next round's fold, `count`
        //     is NOT decremented (it stays in the book), and only the filled slice's margin
        //     is released — the leftover keeps its reservation locked.
        let fully_filled = fill == order.remaining;
        let expired = order.expires_at_auction != 0 && order.expires_at_auction <= auction_id;

        let mut updated = order;
        updated.remaining = new_remaining;

        if fully_filled || expired {
            updated.status = OrderStatus::Consumed as u8;
            updated.reserved_margin = 0;
            release_amount = order.reserved_margin;
            write_order(&mut slab_data, capacity, slot, &updated)?;

            let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
            header.set_count(header.count().saturating_sub(1));
        } else {
            // Split the reservation between the filled slice (which becomes a position and
            // is re-margined below) and the resting leftover (which keeps carrying).
            //
            // Release exactly the filled slice's own worst-case initial margin — priced at
            // the FIXED `worst_price` snapshot (≥ the actual clearing price), so the release
            // always fully covers the position's `initial_margin(fill, clearing_price)` lock
            // below and the settle can NEVER revert on that lock (the "only nets a release"
            // invariant, missing-features §1.1). The leftover keeps the remainder. Because
            // `initial_margin` rounds each slice UP, the leftover's carried reservation is at
            // most ~1 base unit below its standalone worst case — conservative in the safe
            // direction, never a shortfall on a live position. `min` guards a reduce-only
            // order that reserved less than the full quantity (never release more than locked).
            let filled_margin = initial_margin(fill, order.worst_price, initial_bps);
            release_amount = order.reserved_margin.min(filled_margin);
            updated.reserved_margin = order.reserved_margin - release_amount;
            updated.status = OrderStatus::Resting as u8;
            // Per-round fold prefix; cleared so next round's process_chunk re-snapshots it.
            updated.cum_before = 0;
            write_order(&mut slab_data, capacity, slot, &updated)?;

            // Design Z (DDR-1): the order is `Resting` again → it must be re-folded next
            // round, so bump this shard's own `resting_count` (keeper hint) in the same
            // borrow as the status write. `count` is left unchanged — the order stays live.
            let header = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
            let rc = header
                .resting_count()
                .checked_add(1)
                .ok_or(TempoProgramError::MathOverflow)?;
            header.set_resting_count(rc);
        }
    }

    // --- release the filled slice's reserved margin (missing-features §1.1) ---
    //
    // Done BEFORE the position re-lock below so the freed collateral backs the
    // actual margin. The actual lock is always ≤ this reservation (the order was
    // reserved at its worst-case fill price/qty), so settlement only ever NETS a
    // release — it can never revert for lack of collateral, which would wedge the
    // round. Stage B: a fully-filled/expired order releases its FULL leftover
    // reservation (it leaves the book); a re-armed resting order releases only the
    // filled slice and keeps the leftover locked, so `release_amount` may be 0 (a
    // zero-fill order that re-rests touches no collateral and needs no
    // `user_collateral`).
    if release_amount > 0 {
        let uc_acct = ix
            .accounts
            .user_collateral
            .ok_or(TempoProgramError::MissingSettleAccounts)?;
        crate::settle_money::release_order_reservation(
            uc_acct,
            program_id,
            &order_trader,
            release_amount,
        )?;
    }

    // --- record the matched trade ---
    //
    // A non-zero fill is NEVER discarded. The moment a fill exists the
    // trader's `position` account is mandatory and the fill is applied to it —
    // otherwise a (permissionless) cranker could consume the order with empty
    // accounts and silently destroy the matched trade. The `position` is optional
    // in the wire layout only so a *zero-fill* order (it matched nothing) can be
    // consumed cheaply.
    //
    // The collateral ledger + vault remain opt-in: when `user_collateral` is also
    // supplied the realized PnL is flushed to the owner's withdrawable balance and
    // margin is re-locked to the new size (lock more if it grew, release if it
    // shrank); the `vault` is required only when a protocol fee actually applies.
    if fill > 0 {
        let position_acct = ix
            .accounts
            .position
            .ok_or(TempoProgramError::MissingSettleAccounts)?;

        let is_buy = OrderSide::from_u8(order_side)? == OrderSide::Buy;

        // Settle funding, update size/entry, realize PnL on any closed portion.
        // Capture the post-fill open exposure for re-margining.
        let (new_abs_size, new_entry, oi_old, oi_new, is_cross) = {
            let mut acct = *position_acct;
            let mut pos_data = acct.try_borrow_mut()?;
            let position = Position::from_bytes_mut(&mut pos_data)?;
            if position.owner != order_trader || position.market != market_key {
                return Err(TempoProgramError::InvalidOrderOwner.into());
            }
            position.validate_self(position_acct, program_id)?;
            position.settle_funding(funding_index)?;
            // Realize socialized loss on the pre-fill exposure, then
            // re-snapshot a freshly opened position so it never pays loss
            // socialized before it existed.
            position.settle_social_loss(social_long, social_short)?;
            let oi_old = position.size() as i128;
            position.apply_fill(is_buy, fill, settle_price, social_long, social_short)?;
            let oi_new = position.size() as i128;
            (
                position.size().unsigned_abs(),
                position.entry_price(),
                oi_old,
                oi_new,
                position.margin_mode == 1,
            )
        };

        // Keep the market's open-interest totals in step with the fill.
        {
            let mut market_account = *ix.accounts.market;
            let mut market_data = market_account.try_borrow_mut()?;
            Market::from_bytes_mut(&mut market_data)?.apply_oi_delta(oi_old, oi_new);
        }

        // A margin-enabled market (maintenance_bps > 0) REQUIRES the collateral
        // ledger on every non-zero fill, so a position can never grow without
        // locked margin. A no-margin market (maintenance_bps == 0, the clearing
        // benchmark) keeps the ledger optional.
        let uc_opt = match ix.accounts.user_collateral {
            Some(uc) => Some(uc),
            None if maintenance_bps == 0 => None,
            None => return Err(TempoProgramError::MissingSettleAccounts.into()),
        };
        if let Some(uc_acct) = uc_opt {
            // Lock the INITIAL-margin buffer (≥ maintenance) so the position never
            // settles onto its own liquidation line (missing-features §1.2).
            let target_margin = initial_margin(new_abs_size, new_entry, initial_bps);
            // Signed protocol fee on this fill's notional. A `submit_order` fill is
            // always a taker (§1.3), so it pays `taker_fee_bps` (negative = rebate);
            // the `maker_fee_bps` tier applies only on the `settle_maker_quote` path.
            // Routed to the vault fee/insurance pool below; a rebate is funded from it.
            let mut fee = signed_protocol_fee(fill, settle_price, taker_fee_bps);
            // A rebate (negative fee) is funded FROM insurance and must never mint
            // money: clamp its magnitude to the available pool (0 if no vault).
            if fee < 0 {
                let avail = match ix.accounts.vault {
                    Some(v_acct) => {
                        let v_data = v_acct.try_borrow()?;
                        Vault::from_bytes(&v_data)?.insurance_balance() as i128
                    }
                    None => 0,
                };
                if -fee > avail {
                    fee = -avail;
                }
            }

            // Drain the realized cash now that it will actually be flushed.
            let realized = {
                let mut acct = *position_acct;
                let mut pos_data = acct.try_borrow_mut()?;
                let position = Position::from_bytes_mut(&mut pos_data)?;
                let realized = position.realized_pnl();
                position.set_realized_pnl(0);
                realized
            };

            // Flush realized PnL net of fee to the trader's ledger; capture the
            // actual balance change so insurance can absorb the opposite
            // (conservation), and any uncovered loss (bad debt) so it is never
            // silently dropped.
            let (balance_delta, shortfall) = {
                let mut uc = *uc_acct;
                let mut uc_data = uc.try_borrow_mut()?;
                let user_collateral = UserCollateral::from_bytes_mut(&mut uc_data)?;
                if user_collateral.owner != order_trader {
                    return Err(TempoProgramError::InvalidOrderOwner.into());
                }
                user_collateral.validate_self(uc_acct, program_id)?;

                let before = user_collateral.balance();
                let shortfall = user_collateral.apply_pnl(realized - fee)?;
                let balance_delta = user_collateral.balance() as i128 - before as i128;

                // Re-lock isolated margin to the new size (free<->locked only; no
                // balance change). Cross positions lock nothing here — their backing
                // is the combined-equity check in withdraw_cross / liquidate_cross.
                if !is_cross {
                    let current = {
                        let mut pos_acct = *position_acct;
                        let mut pos_data = pos_acct.try_borrow_mut()?;
                        Position::from_bytes_mut(&mut pos_data)?.collateral()
                    };
                    if target_margin > current {
                        user_collateral.lock(target_margin - current)?;
                    } else if current > target_margin {
                        user_collateral.release(current - target_margin);
                    }
                }
                (balance_delta, shortfall)
            };

            if !is_cross {
                let mut pos_acct = *position_acct;
                let mut pos_data = pos_acct.try_borrow_mut()?;
                Position::from_bytes_mut(&mut pos_data)?.set_collateral(target_margin);
            }

            // Conserve the money path through insurance: whatever entered or left
            // the trader's balance (realized PnL net of fee) moves the opposite
            // way in the vault insurance pool, so `vault tokens >= Σ balances +
            // insurance` always holds. A gain is funded from insurance (fail closed
            // if short — never mint money); a loss and the fee accrue to it, and bad
            // debt beyond the balance is socialized to the winning side via the
            // shared settle-money path (symmetric with liquidate, §1.1/§1.2).
            if balance_delta != 0 || shortfall > 0 {
                let vault_acct = ix
                    .accounts
                    .vault
                    .ok_or(TempoProgramError::MissingSettleAccounts)?;
                let mut v = *vault_acct;
                let mut v_data = v.try_borrow_mut()?;
                let vault = Vault::from_bytes_mut(&mut v_data)?;
                vault.validate_self(vault_acct, program_id)?;
                if market_collateral_mint != Address::new_from_array([0u8; 32])
                    && vault.collateral_mint != market_collateral_mint
                {
                    return Err(TempoProgramError::AccountMarketMismatch.into());
                }
                let mut m = *ix.accounts.market;
                let mut m_data = m.try_borrow_mut()?;
                let market = Market::from_bytes_mut(&mut m_data)?;
                // Socialize against the PRE-fill signed size (`oi_old`): the loss
                // occurred on the position's side *before* this fill, so the cohort to
                // charge is chosen from `oi_old`, not the post-fill `oi_new` (which is
                // 0 on a close, or flipped on a flip — charging the wrong side). This
                // matches `liquidate`, which passes the pre-close size (CR-4).
                crate::settle_money::conserve_and_socialize(
                    vault,
                    market,
                    balance_delta,
                    shortfall,
                    oi_old,
                )?;
            }

            // Integrator revenue share: pay a configured cut of a positive,
            // fully-collected fee from the insurance pool to the integrator's
            // ledger (conserving — insurance → integrator, both inside the vault).
            if fee > 0 && shortfall == 0 && integrator_share_bps > 0 {
                if let Some(intg_acct) = ix.accounts.integrator_collateral {
                    let share = (fee as u128).saturating_mul(integrator_share_bps as u128) / 10_000;
                    if share > 0 {
                        let vault_acct = ix
                            .accounts
                            .vault
                            .ok_or(TempoProgramError::MissingSettleAccounts)?;
                        let paid = {
                            let mut v = *vault_acct;
                            let mut v_data = v.try_borrow_mut()?;
                            let vault = Vault::from_bytes_mut(&mut v_data)?;
                            vault.validate_self(vault_acct, program_id)?;
                            let pay = u64::try_from(share)
                                .unwrap_or(u64::MAX)
                                .min(vault.insurance_balance());
                            vault.set_insurance_balance(vault.insurance_balance() - pay);
                            pay
                        };
                        if paid > 0 {
                            let mut ic = *intg_acct;
                            let mut ic_data = ic.try_borrow_mut()?;
                            let ledger = UserCollateral::from_bytes_mut(&mut ic_data)?;
                            ledger.validate_self(intg_acct, program_id)?;
                            ledger.credit(paid)?;
                        }
                    }
                }
            }
        }
    }

    // Emit event via CPI (carries order id + fill; no log!, §1).
    let event = FillSettledEvent {
        market: market_key,
        trader: order_trader,
        order_id: ix.data.order_id,
        auction_id,
        fill,
        side: order_side,
        // A settle_fill fill is always a taker (§1.3); the maker tier reports
        // is_maker=1 only from the settle_maker_quote path.
        is_maker: 0,
        shard_id: order_shard_id,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
