use pinocchio::{
    account::AccountView,
    cpi::{Seed, Signer},
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};
use pinocchio_token::instructions::Transfer;

use crate::{
    cross_margin::{leg_contribution, Leg},
    errors::TempoProgramError,
    instructions::WithdrawCross,
    oracle::{solvency_mark, PYTH_RECEIVER_ID},
    state::{MarginAccount, Market, Position, UserCollateral, Vault},
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes WithdrawCross: a cross-margin extraction. The signer supplies
/// EVERY member position + its market (completeness — omitting a losing leg fails
/// closed). The withdrawal is allowed only if, afterwards, the account's combined
/// equity still covers its combined maintenance — where equity counts collateral +
/// each position's realized PnL + only its *negative* unrealized (losses). Unbacked
/// unrealized GAINS are never credited (preserving the backed-profit rule), so
/// cross-margin nets losses + maintenance across positions without paying out paper
/// profit. Each member market must have a fresh effective price (freshness gate).
pub fn process_withdraw_cross(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = WithdrawCross::try_from((instruction_data, accounts))?;
    let amount = ix.data.amount;
    let owner = *ix.accounts.owner.address();

    // Group: owner + the exact ordered member set.
    let (count, members) = {
        let data = ix.accounts.margin_account.try_borrow()?;
        let margin = MarginAccount::from_bytes(&data)?;
        margin.validate_self(ix.accounts.margin_account, program_id)?;
        if margin.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        let mut keys = alloc::vec::Vec::with_capacity(margin.position_count as usize);
        for i in 0..margin.position_count as usize {
            keys.push(margin.member(i).unwrap());
        }
        (margin.position_count as usize, keys)
    };

    // Completeness: one entry per member, in order. A *live* member is a
    // `(position, market, oracle)` triple (the oracle is the market's bound Pyth
    // account; known-issues §2.2); a *flat* member (size 0) is a bare `position`
    // account — it contributes no unrealized PnL / maintenance and needs no market
    // or oracle, so it does not cost the extra two accounts (known-issues §2.4).
    // `live_mask` bit `i` declares member `i`'s shape; the supplied slice length
    // must match exactly so the cursor walk below can never index out of bounds.
    let live_mask = ix.data.live_mask;
    let live_count = (0..count).filter(|&i| (live_mask >> i) & 1 == 1).count();
    let expected = live_count * 3 + (count - live_count);
    if ix.accounts.members.len() != expected {
        return Err(TempoProgramError::IncompletePortfolio.into());
    }

    let clock = Clock::get()?;
    let now_ts = clock.unix_timestamp;
    let now_slot = clock.slot;

    // Build the combined view: Σ maintenance and Σ (realized + min(0, unrealized)).
    let mut combined_maintenance: i128 = 0;
    let mut recognized: i128 = 0; // Σ realized + Σ losses (never gains)
    let mut member_locked: u64 = 0; // Σ member position margin, to isolate foreign locked
    let mut cursor = 0usize;
    for (i, member_key) in members.iter().enumerate() {
        let is_live = (live_mask >> i) & 1 == 1;
        let position_ai = &ix.accounts.members[cursor];
        if position_ai.address() != member_key {
            return Err(TempoProgramError::IncompletePortfolio.into());
        }

        let (size, entry, realized, pos_market, pos_collateral, pos_funding_ckpt, pos_social_ckpt) = {
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
                position.collateral(),
                position.last_funding_index(),
                position.last_social_index(),
            )
        };
        member_locked = member_locked.saturating_add(pos_collateral);

        if !is_live {
            // Flat leg: zero size means zero unrealized PnL, zero maintenance, and
            // zero unsettled funding/social — so it needs no market or oracle, only
            // its stored realized PnL. A non-flat leg supplied as flat would hide its
            // loss + maintenance, so fail closed (known-issues §2.4).
            if size != 0 {
                return Err(TempoProgramError::IncompletePortfolio.into());
            }
            recognized = recognized.saturating_add(realized);
            cursor += 1;
            continue;
        }

        let market_ai = &ix.accounts.members[cursor + 1];
        let oracle_ai = &ix.accounts.members[cursor + 2];
        cursor += 3;

        // Read the market params + its oracle binding, then price combined health
        // off the RAW per-leg oracle (known-issues §2.2) via the shared
        // `solvency_mark` — the braked effective price (`risk_price`) would let a
        // stale-favorable mark inflate equity and permit over-withdrawal during a
        // crash. A 0/hard-stale oracle cannot back the check (it fails closed).
        let (
            oracle_key,
            feed_id,
            eff_price,
            last_good,
            soft_stale,
            bps,
            mkt_funding,
            mkt_social_long,
            mkt_social_short,
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

        // Dock funding + socialized loss accrued but not yet settled on this leg, so
        // a debt on a read-only leg cannot be withdrawn against (known-issues §1.4).
        let pending = crate::funding::funding_payment(size, mkt_funding, pos_funding_ckpt)?
            .saturating_add(crate::state::pending_social_loss(
                size,
                mkt_social_long,
                mkt_social_short,
                pos_social_ckpt,
            ));
        // Withdrawal applies the backed-profit rule — only *losses* dock equity; an
        // unbacked paper gain is never credited toward what may be pulled out
        // (`credit_unrealized_gains = false`; the shared per-leg primitive, §2.9b).
        let c = leg_contribution(Leg { size, entry, mark }, bps, realized, pending, false);
        recognized = recognized.saturating_add(c.equity);
        combined_maintenance = combined_maintenance.saturating_add(c.maintenance);
    }

    // Debit the shared ledger; require the post-withdraw recognized equity to still
    // cover combined maintenance.
    let (authority_bump, collateral_mint) = {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        (vault.authority_bump, vault.collateral_mint)
    };

    // HS-12: the destination token account must hold the vault's collateral mint.
    {
        let user_token =
            pinocchio_token::state::Account::from_account_view(ix.accounts.user_token_account)?;
        if *user_token.mint() != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        // CR-3: the shared ledger must be scoped to the vault's mint.
        if uc.owner != owner || uc.collateral_mint != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        uc.validate_self(ix.accounts.user_collateral, program_id)?;
        // Margin locked for positions NOT in this group must stay backed. The group's
        // summed member collateral can never exceed the ledger's total `locked`; if it
        // does, the member set and the ledger have drifted — surface that loud rather
        // than clamp to 0 (which would free margin reserved for non-group positions and
        // under-collateralize them — known-issues §2.8).
        let foreign_locked = uc
            .locked()
            .checked_sub(member_locked)
            .ok_or(TempoProgramError::CollateralLedgerDrift)? as i128;
        let balance = uc.balance();
        if amount > balance {
            return Err(TempoProgramError::InsufficientCollateral.into());
        }
        let equity_after = (balance - amount) as i128 + recognized;
        if equity_after < combined_maintenance + foreign_locked {
            return Err(TempoProgramError::InsufficientCollateral.into());
        }
        uc.set_balance(balance - amount);
    }

    let bump = [authority_bump];
    let signer_seeds: [Seed; 2] = [Seed::from(Vault::AUTHORITY_PREFIX), Seed::from(&bump)];
    let signer = Signer::from(&signer_seeds);
    Transfer::new(
        ix.accounts.vault_token_account,
        ix.accounts.user_token_account,
        ix.accounts.vault_authority,
        amount,
    )
    .invoke_signed(&[signer])?;

    Ok(())
}
