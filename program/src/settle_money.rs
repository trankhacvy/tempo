//! Shared settlement/close money path: conserve a balance move through the
//! insurance pool and socialize any uncovered bad debt to the winning side.
//! Single source of truth for `settle_fill`, `settle_maker_quote`, `liquidate`,
//! and `liquidate_cross` so the four paths cannot drift (known-issues §1.1, §1.2).

use pinocchio::{account::AccountView, error::ProgramError, Address, ProgramResult};
use pinocchio_log::log;

use crate::{
    errors::TempoProgramError,
    state::{Market, UserCollateral, Vault},
    traits::{AccountDeserialize, PdaAccount},
};

/// Release an order's worst-case margin reservation (missing-features §1.1) back to
/// the owner's free balance. Shared by `cancel_order` and `settle_fill` so the two
/// release sites cannot drift (a future change to how a reservation is released must
/// touch only this one place). Validates the ledger is the owner's before releasing;
/// `release` is saturating, so an over-release can never underflow.
pub fn release_order_reservation(
    account: &AccountView,
    program_id: &Address,
    owner: &Address,
    amount: u64,
) -> Result<(), ProgramError> {
    if amount == 0 {
        return Ok(());
    }
    let mut acct = *account;
    let mut data = acct.try_borrow_mut()?;
    let uc = UserCollateral::from_bytes_mut(&mut data)?;
    if uc.owner != *owner {
        return Err(TempoProgramError::InvalidOrderOwner.into());
    }
    uc.validate_self(account, program_id)?;
    uc.release(amount);
    Ok(())
}

/// Mirror a user-balance change into the vault's `total_user_balance` aggregate
/// (plan.md §3.4, missing-features §4.2). Call it beside EVERY
/// `UserCollateral.balance` mutation (credit/debit/apply_pnl/set_balance) —
/// `lock`/`release`/`lock_up_to` move `locked` only and are exempt. Checked sub:
/// an over-subtract means the aggregate drifted — surface it as the invariant
/// error (fail closed at the money boundary) rather than wrapping.
pub fn apply_user_balance_delta(vault: &mut Vault, delta: i128) -> Result<(), ProgramError> {
    if delta == 0 {
        return Ok(());
    }
    let cur = vault.total_user_balance();
    let new = if delta > 0 {
        cur.checked_add(delta as u128)
            .ok_or(TempoProgramError::MathOverflow)?
    } else {
        cur.checked_sub((-delta) as u128)
            .ok_or(TempoProgramError::VaultInvariantViolated)?
    };
    vault.set_total_user_balance(new);
    Ok(())
}

/// Conserve `balance_delta` (how the trader's ledger moved, net of fee) against
/// the insurance pool, then socialize any `shortfall` (loss uncovered by the
/// trader's balance) to the winning side by open interest. `loser_signed_size`
/// is the losing position's current signed size and selects which side absorbs
/// the ADL charge. Fails closed (`InsuranceInsolvent`) on an underfunded gain —
/// it never mints money. Also mirrors `balance_delta` into the vault's
/// `total_user_balance` aggregate (§3.4), so every conserving settle keeps the
/// backing invariant checkable on-chain.
pub fn conserve_and_socialize(
    vault: &mut Vault,
    market: &mut Market,
    balance_delta: i128,
    shortfall: u64,
    loser_signed_size: i128,
) -> Result<(), ProgramError> {
    apply_user_balance_delta(vault, balance_delta)?;
    // The insurance pool BEFORE this event accrues the covered loss. It is the
    // baseline the winner's later gain draws from, so the ADL residual is the bad
    // debt beyond it (mirrors liquidate's `bad_debt.saturating_sub(insurance)`).
    let insurance_before = vault.insurance_balance();

    if balance_delta > 0 {
        let need = balance_delta as u64;
        if need > insurance_before {
            return Err(TempoProgramError::InsuranceInsolvent.into());
        }
        vault.set_insurance_balance(insurance_before - need);
    } else if balance_delta < 0 {
        vault.set_insurance_balance(
            insurance_before
                .checked_add((-balance_delta) as u64)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
    }

    // Uncovered loss (only ever co-occurs with balance_delta <= 0): insurance
    // absorbs what it had; the part beyond it is socialized to the winning side
    // (never silently dropped). Insurance is NOT drawn here — the winner draws it
    // when they settle their gain.
    if shortfall > 0 {
        let residual = shortfall.saturating_sub(insurance_before);
        if residual > 0 && !market.socialize_bad_debt(loser_signed_size, residual)? {
            log!("tempo: settle unbacked bad debt={}", residual);
        }
    }
    Ok(())
}

/// Keeper-reward floor (missing-features §6.2): top the equity-capped penalty
/// up to `floor` FROM INSURANCE, capped at the pool (conserving, fail-soft) —
/// the `finalize_clear` crank-fee shape. Griefing-safe by construction: a
/// liquidation only executes when equity < maintenance, an on-chain condition
/// an attacker cannot manufacture for free.
pub fn pay_reward_floor(
    program_id: &Address,
    vault_acct: &AccountView,
    liquidator_ledger: &AccountView,
    market_collateral_mint: Address,
    floor: u64,
    penalty_paid: u64,
) -> ProgramResult {
    if floor <= penalty_paid {
        return Ok(());
    }
    let top_up = {
        let mut v = *vault_acct;
        let mut vault_data = v.try_borrow_mut()?;
        let vault = Vault::from_bytes_mut(&mut vault_data)?;
        vault.validate_self(vault_acct, program_id)?;
        if market_collateral_mint != Address::new_from_array([0u8; 32])
            && vault.collateral_mint != market_collateral_mint
        {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        let pay = (floor - penalty_paid).min(vault.insurance_balance());
        vault.set_insurance_balance(vault.insurance_balance() - pay);
        // §3.4: the liquidator ledger credit below raises Σ user balances.
        apply_user_balance_delta(vault, pay as i128)?;
        pay
    };
    if top_up > 0 {
        let mut acct = *liquidator_ledger;
        let mut lc_data = acct.try_borrow_mut()?;
        UserCollateral::from_bytes_mut(&mut lc_data)?.credit(top_up)?;
    }
    Ok(())
}
