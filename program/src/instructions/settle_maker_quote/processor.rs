use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    clearing::{fill_against_cross, AuctionCross},
    errors::TempoProgramError,
    instructions::SettleMakerQuote,
    margin::{initial_margin, signed_protocol_fee},
    state::{
        AuctionPhase, ClearingResult, MakerQuote, Market, Position, UserCollateral, Vault,
        MAX_LEVELS, SNAPSHOT_UNFOLDED,
    },
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes SettleMakerQuote — Phase 3 SETTLE for a persistent maker quote. Sums
/// the quote's per-level fills (bids at the bid clearing price, asks at the ask
/// clearing price, both uniform) into one net position update + conserving money
/// path. At a rationed marginal tick, each level uses its fold-time `cum_before`
/// snapshot (the maker-region bucket value just before this quote folded into it,
/// recorded by `process_maker_quote`). Because the maker regions are fed only by
/// quotes (taker orders go to the taker regions after §1.3), those snapshots form a
/// contiguous telescoping prefix across ALL makers at the tick, so their fills sum
/// to exactly the allocated volume — no over-allocation when distinct makers share
/// the marginal tick (§1.6). A level whose snapshot is `SNAPSHOT_UNFOLDED` was not
/// folded this round (off-grid or expired) and fills zero.
pub fn process_settle_maker_quote(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = SettleMakerQuote::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // --- market params + phase ---
    let (
        num_ticks,
        auction_id,
        funding_index,
        maint_bps,
        maker_fee_bps,
        market_mint,
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
            market.num_ticks(),
            market.current_auction_id(),
            market.funding_index(),
            market.maintenance_margin_bps(),
            market.maker_fee_bps(),
            market.collateral_mint,
            market.social_loss_index_long(),
            market.social_loss_index_short(),
        )
    };
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        if market.phase == AuctionPhase::Discovered as u8 {
            market.phase = AuctionPhase::Settling as u8;
        }
    }

    // --- clearing result (both auctions) ---
    // `bid`/`ask` are the pure rationing halves fed to the shared classifier;
    // the clearing prices ride alongside for the money path (apply_fill).
    let (bid, bid_price, ask, ask_price) = {
        let cr_data = ix.accounts.clearing_result.try_borrow()?;
        let cr = ClearingResult::from_bytes(&cr_data)?;
        if cr.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        cr.validate_self(ix.accounts.clearing_result, program_id)?;
        if cr.auction_id() != auction_id {
            return Err(TempoProgramError::AuctionIdMismatch.into());
        }
        (
            AuctionCross {
                marginal_tick: cr.bid_marginal_tick(),
                matched_volume: cr.bid_matched_volume(),
                volume_allocated_to_marginal_tick: cr.bid_volume_allocated_to_marginal_tick(),
                total_qty_at_marginal_tick: cr.bid_total_qty_at_marginal_tick(),
                rationed_side: cr.bid_rationed_side,
            },
            cr.bid_clearing_price(),
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

    // --- read the quote (settle-once + folded guards) ---
    let maker;
    let mid_tick;
    let num_bids;
    let num_asks;
    let mut bid_levels = [(0u16, 0u64); MAX_LEVELS];
    let mut ask_levels = [(0u16, 0u64); MAX_LEVELS];
    // Per-level `cum_before`, captured at fold (process_maker_quote, §1.6). This is
    // the conserving telescoping prefix across ALL makers at a shared marginal tick:
    // a maker no longer assumes it is the only one there. SNAPSHOT_UNFOLDED marks a
    // level the histogram never saw (off-grid / expired) → it fills zero.
    let mut bid_snaps = [SNAPSHOT_UNFOLDED; MAX_LEVELS];
    let mut ask_snaps = [SNAPSHOT_UNFOLDED; MAX_LEVELS];
    {
        let q_data = ix.accounts.maker_quote.try_borrow()?;
        let q = MakerQuote::from_bytes(&q_data)?;
        if q.market != market_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        q.validate_self(ix.accounts.maker_quote, program_id)?;
        if q.settled_auction_id() == auction_id {
            return Err(TempoProgramError::InvalidOrderStatus.into()); // already settled
        }
        if q.folded_auction_id() != auction_id {
            return Err(TempoProgramError::AuctionNotComplete.into()); // not folded this round
        }
        maker = q.maker;
        mid_tick = q.mid_tick();
        num_bids = q.num_bids as usize;
        num_asks = q.num_asks as usize;
        for (i, slot) in bid_levels.iter_mut().enumerate().take(num_bids) {
            *slot = q.bid_level(i);
        }
        for (i, slot) in ask_levels.iter_mut().enumerate().take(num_asks) {
            *slot = q.ask_level(i);
        }
        for (i, slot) in bid_snaps.iter_mut().enumerate().take(num_bids) {
            *slot = q.bid_snapshot(i);
        }
        for (i, slot) in ask_snaps.iter_mut().enumerate().take(num_asks) {
            *slot = q.ask_snapshot(i);
        }
    }

    // The order_slab account is retained in the layout but no longer scanned: after
    // §1.3 the maker regions are fed exclusively by quotes, so the marginal-tick
    // prefix comes entirely from the fold snapshots above, not from resting orders.

    // --- sum the quote's fills (bids at bid price, asks at ask price) ---
    // Each level uses its own fold snapshot as `cum_before`; a sentinel snapshot
    // means the level was not folded this round and contributes nothing.
    let mut total_bid_fill = 0u64;
    for (i, (offset, size)) in bid_levels.iter().take(num_bids).enumerate() {
        let snap = bid_snaps[i];
        if snap == SNAPSHOT_UNFOLDED {
            continue;
        }
        let Some(tick) = mid_tick.checked_sub(*offset as u32) else {
            continue;
        };
        let fill = fill_against_cross(&bid, true, tick, *size, snap)?;
        total_bid_fill = total_bid_fill
            .checked_add(fill)
            .ok_or(TempoProgramError::MathOverflow)?;
    }
    let mut total_ask_fill = 0u64;
    for (i, (offset, size)) in ask_levels.iter().take(num_asks).enumerate() {
        let snap = ask_snaps[i];
        if snap == SNAPSHOT_UNFOLDED {
            continue;
        }
        let Some(tick) = mid_tick
            .checked_add(*offset as u32)
            .filter(|t| *t < num_ticks)
        else {
            continue;
        };
        let fill = fill_against_cross(&ask, false, tick, *size, snap)?;
        total_ask_fill = total_ask_fill
            .checked_add(fill)
            .ok_or(TempoProgramError::MathOverflow)?;
    }

    // --- mark settled-this-round ---
    {
        let mut q_account = *ix.accounts.maker_quote;
        let mut q_data = q_account.try_borrow_mut()?;
        MakerQuote::from_bytes_mut(&mut q_data)?.set_settled_auction_id(auction_id);
    }

    if total_bid_fill == 0 && total_ask_fill == 0 {
        return Ok(());
    }

    // --- money path (mirrors settle_fill, with two uniform-price fills) ---
    let new_abs_size;
    let new_entry;
    let oi_old;
    let oi_new;
    {
        let mut acct = *ix.accounts.position;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        if position.owner != maker || position.market != market_key {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        position.validate_self(ix.accounts.position, program_id)?;
        position.settle_funding(funding_index)?;
        position.settle_social_loss(social_long, social_short)?;
        oi_old = position.size() as i128;
        if total_bid_fill > 0 {
            position.apply_fill(true, total_bid_fill, bid_price, social_long, social_short)?;
        }
        if total_ask_fill > 0 {
            position.apply_fill(false, total_ask_fill, ask_price, social_long, social_short)?;
        }
        oi_new = position.size() as i128;
        new_abs_size = position.size().unsigned_abs();
        new_entry = position.entry_price();
    }

    // Keep the market's open-interest totals in step with the quote fills.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        Market::from_bytes_mut(&mut market_data)?.apply_oi_delta(oi_old, oi_new);
    }

    let uc_opt = match ix.accounts.user_collateral {
        Some(uc) => Some(uc),
        None if maint_bps == 0 => None,
        None => return Err(TempoProgramError::MissingSettleAccounts.into()),
    };
    if let Some(uc_acct) = uc_opt {
        // Maker quotes have NO pre-trade reservation (unlike `submit_order`, §1.1),
        // so lock only the MAINTENANCE margin here — requiring the higher initial
        // buffer with nothing reserved could revert and wedge a maker who can't top
        // up (a permissionless cranker can't fix it for them). The initial-margin
        // buffer is a taker-path guarantee; makers get it once quote-time margin
        // (missing-features §7.1) is built.
        let target_margin = initial_margin(new_abs_size, new_entry, maint_bps);
        // Maker fee on both sides' notional (signed: negative = rebate).
        let mut fee = signed_protocol_fee(total_bid_fill, bid_price, maker_fee_bps)
            .checked_add(signed_protocol_fee(
                total_ask_fill,
                ask_price,
                maker_fee_bps,
            ))
            .ok_or(TempoProgramError::MathOverflow)?;
        // A rebate is funded from insurance and must never mint money.
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

        let realized = {
            let mut acct = *ix.accounts.position;
            let mut pos_data = acct.try_borrow_mut()?;
            let position = Position::from_bytes_mut(&mut pos_data)?;
            let realized = position.realized_pnl();
            position.set_realized_pnl(0);
            realized
        };

        let (balance_delta, shortfall) = {
            let mut uc = *uc_acct;
            let mut uc_data = uc.try_borrow_mut()?;
            let user_collateral = UserCollateral::from_bytes_mut(&mut uc_data)?;
            if user_collateral.owner != maker {
                return Err(TempoProgramError::InvalidOrderOwner.into());
            }
            user_collateral.validate_self(uc_acct, program_id)?;

            let before = user_collateral.balance();
            let shortfall = user_collateral.apply_pnl(realized - fee)?;
            let balance_delta = user_collateral.balance() as i128 - before as i128;

            let current = {
                let mut pos_acct = *ix.accounts.position;
                let mut pos_data = pos_acct.try_borrow_mut()?;
                Position::from_bytes_mut(&mut pos_data)?.collateral()
            };
            if target_margin > current {
                user_collateral.lock(target_margin - current)?;
            } else if current > target_margin {
                user_collateral.release(current - target_margin);
            }
            (balance_delta, shortfall)
        };

        {
            let mut pos_acct = *ix.accounts.position;
            let mut pos_data = pos_acct.try_borrow_mut()?;
            Position::from_bytes_mut(&mut pos_data)?.set_collateral(target_margin);
        }

        if balance_delta != 0 || shortfall > 0 {
            let vault_acct = ix
                .accounts
                .vault
                .ok_or(TempoProgramError::MissingSettleAccounts)?;
            let mut v = *vault_acct;
            let mut v_data = v.try_borrow_mut()?;
            let vault = Vault::from_bytes_mut(&mut v_data)?;
            vault.validate_self(vault_acct, program_id)?;
            if market_mint != Address::new_from_array([0u8; 32])
                && vault.collateral_mint != market_mint
            {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            // Fail closed on an underfunded gain and socialize bad debt through the
            // shared settle-money path — the maker path no longer mints money (§1.1).
            let mut m = *ix.accounts.market;
            let mut m_data = m.try_borrow_mut()?;
            let market = Market::from_bytes_mut(&mut m_data)?;
            crate::settle_money::conserve_and_socialize(
                vault,
                market,
                balance_delta,
                shortfall,
                oi_new,
            )?;
        }
    }

    Ok(())
}
