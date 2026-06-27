use pinocchio::{account::AccountView, Address, ProgramResult};
use pinocchio_log::log;

use crate::{
    cross_margin::{leg_contribution, Leg},
    errors::TempoProgramError,
    events::PositionLiquidatedEvent,
    instructions::LiquidateCross,
    margin::{is_liquidatable, unrealized_pnl},
    oracle::{solvency_mark, PYTH_RECEIVER_ID},
    state::{MarginAccount, Market, Position, UserCollateral, Vault},
    traits::{AccountDeserialize, EventSerialize, PdaAccount},
    utils::emit_event,
};
use pinocchio::sysvars::{clock::Clock, Sysvar};

/// Processes LiquidateCross: liquidates a cross-margin account that is
/// *combined*-unhealthy by fully closing ONE member (the first supplied pair),
/// using the same conserving per-position close as `liquidate`. The liquidatability
/// gate is the combined equity vs combined maintenance over EVERY member (omitting
/// one fails closed). Closing any non-flat member realizes its PnL and removes its
/// maintenance, so the combined deficit strictly shrinks — repeated calls wind the
/// account down in bounded steps.
pub fn process_liquidate_cross(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = LiquidateCross::try_from((instruction_data, accounts))?;

    // Group: owner + member set.
    let (owner, count, member_keys) = {
        let data = ix.accounts.margin_account.try_borrow()?;
        let margin = MarginAccount::from_bytes(&data)?;
        margin.validate_self(ix.accounts.margin_account, program_id)?;
        let mut keys = alloc::vec::Vec::with_capacity(margin.position_count as usize);
        for i in 0..margin.position_count as usize {
            keys.push(margin.member(i).unwrap());
        }
        (margin.owner, margin.position_count as usize, keys)
    };
    // One entry per member: a *live* member is a `(position, market, oracle)` triple
    // (known-issues §2.2); a *flat* member (size 0) is a bare `position` account — it
    // contributes nothing to combined equity but its realized PnL and needs no market
    // or oracle, so it does not cost the extra two accounts (known-issues §2.4).
    // `live_mask` bit `i` declares the i-th supplied member's shape; the slice length
    // must match exactly so the cursor walk can never index out of bounds.
    let live_mask = ix.data.live_mask;
    let live_count = (0..count).filter(|&i| (live_mask >> i) & 1 == 1).count();
    let expected = live_count * 3 + (count - live_count);
    if count == 0 || ix.accounts.members.len() != expected {
        return Err(TempoProgramError::IncompletePortfolio.into());
    }

    let clock = Clock::get()?;
    let now_ts = clock.unix_timestamp;
    let now_slot = clock.slot;

    let balance = {
        let uc_data = ix.accounts.user_collateral.try_borrow()?;
        let uc = UserCollateral::from_bytes(&uc_data)?;
        if uc.owner != owner {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        uc.balance()
    };

    // Combined health over every member; the close target is the first *non-flat*
    // supplied member (a flat leg has nothing to close).
    let mut combined_equity: i128 = balance as i128;
    let mut combined_maintenance: i128 = 0;
    let mut target = None;
    let mut seen: alloc::vec::Vec<&Address> = alloc::vec::Vec::with_capacity(count);
    let mut cursor = 0usize;
    for i in 0..count {
        let is_live = (live_mask >> i) & 1 == 1;
        let position_ai = &ix.accounts.members[cursor];
        let pk = position_ai.address();
        if !member_keys.iter().any(|k| k == pk) {
            return Err(TempoProgramError::IncompletePortfolio.into());
        }
        if seen.contains(&pk) {
            return Err(TempoProgramError::IncompletePortfolio.into());
        }
        seen.push(pk);

        let (size, entry, realized, pos_market, pos_funding_ckpt, pos_social_ckpt) = {
            let pos_data = position_ai.try_borrow()?;
            let position = Position::from_bytes(&pos_data)?;
            position.validate_self(position_ai, program_id)?;
            if position.owner != owner {
                return Err(TempoProgramError::InvalidOrderOwner.into());
            }
            (
                position.size() as i128,
                position.entry_price(),
                position.realized_pnl(),
                position.market,
                position.last_funding_index(),
                position.last_social_index(),
            )
        };

        if !is_live {
            // Flat leg: zero size → zero unrealized, zero maintenance, zero unsettled
            // funding/social. Counts only its realized PnL toward combined equity and
            // needs no market/oracle. A non-flat leg supplied as flat would hide its
            // loss + maintenance from the gate, so fail closed (known-issues §2.4).
            if size != 0 {
                return Err(TempoProgramError::IncompletePortfolio.into());
            }
            combined_equity = combined_equity.saturating_add(realized);
            cursor += 1;
            continue;
        }

        let market_ai = &ix.accounts.members[cursor + 1];
        let oracle_ai = &ix.accounts.members[cursor + 2];
        cursor += 3;

        // Read the market params + its oracle binding, then price solvency off the
        // RAW per-leg oracle (known-issues §2.2) via the shared `solvency_mark` — the
        // braked effective price (`risk_price`) would let the per-slot brake delay a
        // crash liquidation. Every leg (not just the target) is priced raw so a
        // stale-favorable read-only leg cannot inflate combined equity.
        let (
            oracle_key,
            feed_id,
            eff_price,
            last_good,
            soft_stale,
            bps,
            funding_index,
            social_long,
            social_short,
            penalty_bps,
            mint,
        ) = {
            let market_data = market_ai.try_borrow()?;
            let market = Market::from_account(&market_data, market_ai, program_id)?;
            if *market_ai.address() != pos_market {
                return Err(TempoProgramError::AccountMarketMismatch.into());
            }
            (
                market.oracle,
                market.oracle_feed_id,
                market.effective_price_1e8(),
                market.last_good_oracle_slot(),
                market.soft_stale_slots(),
                market.maintenance_margin_bps(),
                market.funding_index(),
                market.social_loss_index_long(),
                market.social_loss_index_short(),
                market.liquidation_penalty_bps(),
                market.collateral_mint,
            )
        };
        if oracle_ai.address() != &oracle_key {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        if !oracle_ai.owned_by(&PYTH_RECEIVER_ID) {
            return Err(TempoProgramError::OracleInvalidAccount.into());
        }
        let mark = {
            let oracle_data = oracle_ai.try_borrow()?;
            solvency_mark(
                &oracle_data,
                &feed_id,
                now_ts,
                now_slot,
                eff_price,
                last_good,
                soft_stale,
            )?
            .price()
        };

        // Dock funding + socialized loss accrued but not yet settled on every leg
        // (including read-only ones) so the combined-health gate sees true equity
        // (known-issues §1.4). The target leg is settled for real before its close.
        let pending =
            crate::funding::funding_payment(size, funding_index, pos_funding_ckpt)?.saturating_add(
                crate::state::pending_social_loss(size, social_long, social_short, pos_social_ckpt),
            );
        // Liquidation marks to the true price — gains and losses both count toward
        // whether the account is underwater (the shared per-leg primitive, §2.9b).
        let c = leg_contribution(Leg { size, entry, mark }, bps, realized, pending, true);
        combined_equity = combined_equity.saturating_add(c.equity);
        combined_maintenance = combined_maintenance.saturating_add(c.maintenance);

        // The first non-flat member is the close target.
        if target.is_none() && size != 0 {
            target = Some((
                position_ai,
                market_ai,
                mark,
                funding_index,
                social_long,
                social_short,
                penalty_bps,
                mint,
            ));
        }
    }

    // The ACCOUNT must be combined-unhealthy.
    if !is_liquidatable(combined_equity, combined_maintenance) {
        return Err(TempoProgramError::NotLiquidatable.into());
    }

    let (
        target_pos,
        target_market,
        mark,
        funding_index,
        social_long,
        social_short,
        penalty_bps,
        market_mint,
    ) = target.ok_or(TempoProgramError::NotLiquidatable)?;
    let market_key = *target_market.address();

    // --- close the target (identical conserving flow to `liquidate`) ---
    let (owner_key, size_signed, entry, collateral, realized) = {
        let mut acct = *target_pos;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        position.settle_funding(funding_index)?;
        position.settle_social_loss(social_long, social_short)?;
        (
            position.owner,
            position.size() as i128,
            position.entry_price(),
            position.collateral(),
            position.realized_pnl(),
        )
    };

    // Cross-margin close: the loss is drawn from the SHARED account balance, not
    // the position's isolated locked margin (that would book spurious bad debt
    // while the account still has free balance). Realize the close PnL
    // (funding/social already in `realized`, plus mark PnL) against the balance,
    // release the target's locked margin, charge the liquidation penalty, and
    // conserve through insurance exactly as `settle_fill` does.
    let pnl = realized
        .checked_add(unrealized_pnl(size_signed, entry, mark))
        .ok_or(TempoProgramError::MathOverflow)?;
    let notional = size_signed.unsigned_abs().saturating_mul(mark as u128);
    let penalty = u64::try_from(
        crate::wide_math::mul_div_floor(notional, penalty_bps as u128, 10_000).unwrap_or(u128::MAX),
    )
    .unwrap_or(u64::MAX);

    let mut social_residual: u64 = 0;
    let position_equity = collateral as i128 + pnl;

    let (balance_delta, shortfall, penalty_charged) = {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        uc.validate_self(ix.accounts.user_collateral, program_id)?;
        // Free the target's reserved margin (free<->locked only; no balance change).
        let release_amt = (collateral as u128).min(uc.locked() as u128) as u64;
        uc.release(release_amt);
        let before = uc.balance();
        let shortfall = uc.apply_pnl(pnl)?;
        let after_pnl = uc.balance();
        let penalty_charged = penalty.min(uc.balance());
        uc.set_balance(uc.balance() - penalty_charged);
        (
            after_pnl as i128 - before as i128,
            shortfall,
            penalty_charged,
        )
    };

    // Penalty moves owner balance -> liquidator ledger (both vault-backed, conserved).
    if penalty_charged > 0 {
        let mut acct = *ix.accounts.liquidator_collateral;
        let mut lc_data = acct.try_borrow_mut()?;
        UserCollateral::from_bytes_mut(&mut lc_data)?.credit(penalty_charged)?;
    }

    {
        let mut acct = *ix.accounts.vault;
        let mut vault_data = acct.try_borrow_mut()?;
        let vault = Vault::from_bytes_mut(&mut vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        if market_mint != Address::new_from_array([0u8; 32]) && vault.collateral_mint != market_mint
        {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        let insurance = vault.insurance_balance();
        // The PnL that entered/left the owner's balance moves the opposite way in
        // insurance. A gain is funded from insurance (fail closed if it cannot,
        // a loss accrues to it; uncovered bad debt is socialized.
        if balance_delta > 0 {
            let need = balance_delta as u64;
            if need > insurance {
                return Err(TempoProgramError::InsuranceInsolvent.into());
            }
            vault.set_insurance_balance(insurance - need);
        } else if balance_delta < 0 {
            vault.set_insurance_balance(
                insurance
                    .checked_add((-balance_delta) as u64)
                    .ok_or(TempoProgramError::MathOverflow)?,
            );
        }
        if shortfall > 0 {
            social_residual = shortfall.saturating_sub(insurance);
        }
    }

    {
        let mut acct = *target_pos;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        position.set_size(0);
        position.set_collateral(0);
        position.set_entry_price(0);
        position.set_realized_pnl(0);
    }

    {
        let mut acct = *target_market;
        let mut market_data = acct.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.apply_oi_delta(size_signed, 0);
        if !market.socialize_bad_debt(size_signed, social_residual)? {
            log!("tempo: xliq unbacked bad debt={}", social_residual);
        }
    }

    let event = PositionLiquidatedEvent {
        market: market_key,
        owner: owner_key,
        mark,
        equity: position_equity,
        penalty: penalty_charged,
        bad_debt: shortfall,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
