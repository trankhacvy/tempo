use pinocchio::{
    account::AccountView,
    cpi::{Seed, Signer},
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};
use pinocchio_token::{instructions::Transfer, state::Account as TokenAccount};

use crate::{
    errors::TempoProgramError,
    events::InsuranceWithdrawnEvent,
    instructions::ApplyInsuranceWithdraw,
    state::Vault,
    traits::{AccountDeserialize, EventSerialize, PdaAccount},
    utils::emit_event,
};

/// Processes ApplyInsuranceWithdraw (plan.md §4.4): PERMISSIONLESS after the
/// staged delay. The pay amount re-clamps to the CURRENT pool (it may have
/// shrunk since propose), and the §4.2 FAIL-CLOSED backing gate runs after the
/// insurance debit, before the transfer — tokens may only leave while the vault
/// still covers every user balance + the remaining pool.
pub fn process_apply_insurance_withdraw(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ApplyInsuranceWithdraw::try_from((instruction_data, accounts))?;
    let now_slot = Clock::get()?.slot;

    let (pay, authority_bump, collateral_mint, vault_authority_key) = {
        let mut acct = *ix.accounts.vault;
        let mut vault_data = acct.try_borrow_mut()?;
        let vault = Vault::from_bytes_mut(&mut vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        let amount = vault.pending_withdraw_amount();
        if amount == 0 {
            return Err(TempoProgramError::NoPendingUpdate.into());
        }
        if now_slot < vault.pending_withdraw_slot() {
            return Err(TempoProgramError::PendingDelayNotElapsed.into());
        }
        // HS-12: the recipient must hold the vault's mint AND be owned by the
        // vault authority. Apply is permissionless (any cranker may execute the
        // staged withdraw so a keeper can), but the proposal records no
        // destination, so without this an attacker could front-run the apply and
        // redirect the authority's staged pool withdrawal to their own same-mint
        // account (theft of insurance funds — the users' backing invariant would
        // stay intact, so no gate would fire). Binding the destination to the
        // authority's own token accounts closes that: the funds can only land
        // where the (delay-gated) authority controls them.
        {
            let recipient = TokenAccount::from_account_view(ix.accounts.recipient_token_account)?;
            if *recipient.mint() != vault.collateral_mint {
                return Err(TempoProgramError::InvalidCollateralAccount.into());
            }
            if *recipient.owner() != vault.authority {
                return Err(TempoProgramError::InvalidAuthority.into());
            }
        }
        // Re-clamp to what the pool holds NOW, debit, clear the staging slot.
        let pay = amount.min(vault.insurance_balance());
        vault.set_insurance_balance(vault.insurance_balance() - pay);
        vault.set_pending_withdraw_amount(0);
        vault.set_pending_withdraw_slot(0);

        // §4.2 FAIL-CLOSED backing gate: post-debit, pre-transfer.
        let vault_token = TokenAccount::from_account_view(ix.accounts.vault_token_account)?;
        let backing_needed = vault
            .total_user_balance()
            .saturating_add(vault.insurance_balance() as u128)
            .saturating_add(pay as u128); // the tokens leaving in this tx
        if (vault_token.amount() as u128) < backing_needed {
            return Err(TempoProgramError::VaultInvariantViolated.into());
        }
        (
            pay,
            vault.authority_bump,
            vault.collateral_mint,
            vault.authority,
        )
    };

    if pay > 0 {
        let bump = [authority_bump];
        let signer_seeds: [Seed; 2] = [Seed::from(Vault::AUTHORITY_PREFIX), Seed::from(&bump)];
        let signer = Signer::from(&signer_seeds);
        Transfer::new(
            ix.accounts.vault_token_account,
            ix.accounts.recipient_token_account,
            ix.accounts.vault_authority,
            pay,
        )
        .invoke_signed(&[signer])?;
    }

    let event = InsuranceWithdrawnEvent {
        collateral_mint,
        authority: vault_authority_key,
        amount: pay,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
