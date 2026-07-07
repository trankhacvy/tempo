use pinocchio::{account::AccountView, Address, ProgramResult};
use pinocchio_log::log;

use crate::{
    errors::TempoProgramError,
    events::PositionLiquidatedEvent,
    instructions::Liquidate,
    margin::{is_liquidatable, liquidation_outcome, maintenance_margin},
    oracle::{solvency_mark, SolvencyMark, PYTH_RECEIVER_ID},
    state::{Market, Position, UserCollateral, Vault},
    traits::{AccountDeserialize, EventSerialize, PdaAccount},
    utils::emit_event,
};
use pinocchio::sysvars::{clock::Clock, Sysvar};

/// Processes the Liquidate instruction (permissionless): closes
/// a position whose equity has fallen below its maintenance margin, oracle-priced.
/// The owner keeps `returned_to_owner`, the liquidator earns `penalty`, and any
/// `bad_debt` is drawn from the vault insurance fund (saturating).
pub fn process_liquidate(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = Liquidate::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    // Read market: bound oracle, feed id, funding index, per-market risk params.
    let (
        oracle_key,
        feed_id,
        funding_index,
        maintenance_bps,
        penalty_bps,
        market_collateral_mint,
        social_long,
        social_short,
        effective_price,
        last_good_oracle_slot,
        soft_stale_slots,
    ) = {
        let market_data = ix.accounts.market.try_borrow()?;
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        (
            market.oracle,
            market.oracle_feed_id,
            market.funding_index(),
            market.maintenance_margin_bps(),
            market.liquidation_penalty_bps(),
            market.collateral_mint,
            market.social_loss_index_long(),
            market.social_loss_index_short(),
            market.effective_price_1e8(),
            market.last_good_oracle_slot(),
            market.soft_stale_slots(),
        )
    };

    // Validate the oracle binding + ownership.
    if ix.accounts.oracle.address() != &oracle_key {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }
    if !ix.accounts.oracle.owned_by(&PYTH_RECEIVER_ID) {
        return Err(TempoProgramError::OracleInvalidAccount.into());
    }

    let clock = Clock::get()?;
    let now_ts = clock.unix_timestamp;
    let now_slot = clock.slot;
    // Solvency prices off the RAW (confidence-checked) oracle, not the braked
    // effective price (known-issues §2.2) — see `oracle::solvency_mark`, the one
    // shared resolver used by `liquidate` and both cross-margin paths. On a fresh
    // print we also advance + persist the braked mark (for funding / the freshness
    // anchor); on a soft-stale oracle we price off the frozen effective price and
    // leave the brake untouched.
    let resolved = {
        let oracle_data = ix.accounts.oracle.try_borrow()?;
        solvency_mark(
            &oracle_data,
            &feed_id,
            now_ts,
            now_slot,
            effective_price,
            last_good_oracle_slot,
            soft_stale_slots,
        )?
    };
    if let SolvencyMark::Fresh(raw) = resolved {
        // Advance the braked mark off the raw price (rolled back with the tx if the
        // position turns out not to be liquidatable).
        let mut acct = *ix.accounts.market;
        let mut md = acct.try_borrow_mut()?;
        Market::from_bytes_mut(&mut md)?.advance_effective_price(raw, now_slot);
    }
    let mark = resolved.price();

    // Settle funding into the position, then read its post-funding state.
    let (owner_key, position_market, locked_release, size_signed, entry, collateral, realized) = {
        let mut acct = *ix.accounts.position;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        position.settle_funding(funding_index)?;
        // Realize any socialized loss already charged to this position's side
        // before pricing the close.
        position.settle_social_loss(social_long, social_short)?;
        (
            position.owner,
            position.market,
            position.collateral(),
            position.size() as i128,
            position.entry_price(),
            position.collateral(),
            position.realized_pnl(),
        )
    };

    // The position must belong to this market.
    if position_market != market_key {
        return Err(TempoProgramError::AccountMarketMismatch.into());
    }

    let outcome = liquidation_outcome(collateral, realized, size_signed, entry, mark, penalty_bps);
    let maint = maintenance_margin(size_signed, mark, maintenance_bps);
    if !is_liquidatable(outcome.equity, maint) {
        return Err(TempoProgramError::NotLiquidatable.into());
    }

    // Bad debt left uncovered by insurance, socialized to the winning side.
    let mut social_residual: u64 = 0;

    // Owner ledger: the position's collateral was locked inside `balance`, so
    // replace it with the post-liquidation residual: balance - collateral +
    // returned_to_owner. The loss + penalty thus leave the owner's claim.
    let owner_balance_delta = {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        if uc.owner != owner_key {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        uc.validate_self(ix.accounts.user_collateral, program_id)?;
        let before = uc.balance();
        uc.release(locked_release);
        // The released collateral leaves `balance`; capture any shortfall instead
        // of dropping it (locked <= balance normally holds, so it is usually 0).
        let owner_shortfall = uc.apply_pnl(-(locked_release as i128))?;
        if owner_shortfall > 0 {
            log!("tempo: liq owner ledger shortfall={}", owner_shortfall);
        }
        if outcome.returned_to_owner > 0 {
            uc.credit(outcome.returned_to_owner)?;
        }
        uc.balance() as i128 - before as i128
    };

    // Pay the penalty to the liquidator's ledger.
    if outcome.penalty > 0 {
        let mut acct = *ix.accounts.liquidator_collateral;
        let mut lc_data = acct.try_borrow_mut()?;
        let lc = UserCollateral::from_bytes_mut(&mut lc_data)?;
        lc.credit(outcome.penalty)?;
    }

    // Conserve the close through insurance: whatever the close moved into/out of
    // the owner + liquidator ledgers moves the opposite way in the pool, so
    // `vault tokens >= Σ balances + insurance` holds. The owner's realized loss
    // accrues to insurance (to fund the counterparty's gain); an owner gain is
    // funded from it. insurance_delta = collateral - returned_to_owner - penalty.
    {
        let mut acct = *ix.accounts.vault;
        let mut vault_data = acct.try_borrow_mut()?;
        let vault = Vault::from_bytes_mut(&mut vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        if market_collateral_mint != Address::new_from_array([0u8; 32])
            && vault.collateral_mint != market_collateral_mint
        {
            return Err(TempoProgramError::AccountMarketMismatch.into());
        }
        // §3.4: mirror the owner's actual balance change + the liquidator's
        // penalty credit into the backing aggregate.
        crate::settle_money::apply_user_balance_delta(
            vault,
            owner_balance_delta + outcome.penalty as i128,
        )?;
        let insurance = vault.insurance_balance();
        let insurance_delta: i128 =
            collateral as i128 - outcome.returned_to_owner as i128 - outcome.penalty as i128;
        let insurance_after = if insurance_delta >= 0 {
            insurance
                .checked_add(insurance_delta as u64)
                .ok_or(TempoProgramError::MathOverflow)?
        } else {
            insurance.saturating_sub((-insurance_delta) as u64)
        };
        // The seized collateral covers the `collateral` slice of the winners'
        // claim; the bad debt is the extra they are owed. Pre-existing insurance
        // (`insurance`, before this seizure) absorbs what it can — those winners
        // draw their full claim from insurance later — and only the part beyond it
        // is socialized to the winning side (never silently dropped).
        if outcome.bad_debt > 0 {
            social_residual = outcome.bad_debt.saturating_sub(insurance);
        }
        vault.set_insurance_balance(insurance_after);
    }

    // Zero out the closed position (realized PnL is reset with the close).
    {
        let mut acct = *ix.accounts.position;
        let mut pos_data = acct.try_borrow_mut()?;
        let position = Position::from_bytes_mut(&mut pos_data)?;
        position.set_size(0);
        position.set_collateral(0);
        position.set_entry_price(0);
        position.set_realized_pnl(0);
    }

    // Drop the closed position's open interest, then socialize any
    // uncovered bad debt to the winning (counterparty) side by its open interest.
    // A liquidated long's residual is charged to shorts, and vice-versa.
    {
        let mut market_account = *ix.accounts.market;
        let mut market_data = market_account.try_borrow_mut()?;
        let market = Market::from_bytes_mut(&mut market_data)?;
        market.apply_oi_delta(size_signed, 0);
        if !market.socialize_bad_debt(size_signed, social_residual)? {
            log!(
                "tempo: liq unbacked bad debt (no winning OI)={}",
                social_residual
            );
        }
    }

    log!(
        "tempo: liquidated penalty={} returned={} bad_debt={}",
        outcome.penalty,
        outcome.returned_to_owner,
        outcome.bad_debt
    );

    let equity_i128 = outcome.equity;
    let event = PositionLiquidatedEvent {
        market: market_key,
        owner: owner_key,
        mark,
        equity: equity_i128,
        penalty: outcome.penalty,
        bad_debt: outcome.bad_debt,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )?;

    Ok(())
}
