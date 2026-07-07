use alloc::vec;
use alloc::vec::Vec;
use codama::CodamaAccount;
use pinocchio::{cpi::Seed, Address};

use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};
use crate::{assert_no_padding, le_field};

/// Global collateral vault (singleton). Holds the program's one
/// collateral mint and tracks the insurance balance.
///
/// # PDA Seeds
/// `[b"vault", collateral_mint]` (per-collateral; multiple mints → multiple vaults)
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1**)
/// 3 × Address (96) + [u8;8] (8) + [u8;16] (16) + 2 × [u8;8] (16) + 2 × u8 (2) = 138.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 6))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "vault"))]
#[codama(seed(name = "collateralMint", type = public_key))]
#[repr(C)]
pub struct Vault {
    pub collateral_mint: Address,
    pub vault_token_account: Address,
    pub insurance_balance_le: [u8; 8],
    /// Bump of the vault authority PDA (`[b"vault_authority"]`) that owns the
    /// token account and signs withdrawals.
    pub authority_bump: u8,
    /// Bump of this Vault PDA.
    pub bump: u8,
    // --- v3 (plan.md §3.4): admin + backing aggregate + staged withdraw ---
    /// Vault admin, recorded at init (v2 had NO stored authority, so the
    /// Phase-3 insurance withdraw had no one to gate on). Vault creation is
    /// first-come per mint (create-once), same trust model as the recorded
    /// `vault_token_account`.
    pub authority: Address,
    /// Running Σ of every `UserCollateral.balance` under this mint (u128). The
    /// conservation-counter exception to the "scans not counters" rule: there
    /// is no on-chain scan alternative, and drift is caught FAIL-CLOSED at the
    /// token-outflow sites (`VaultInvariantViolated` blocks withdrawals — funds
    /// stay safe) rather than wedging rounds (the liveness-counter failure mode
    /// Design Z removed). Maintained by `settle_money::apply_user_balance_delta`.
    pub total_user_balance_le: [u8; 16],
    /// Staged insurance withdraw (plan.md §4.4): amount (0 = none pending).
    pub pending_withdraw_amount_le: [u8; 8],
    /// Slot at which the staged withdraw may be applied (permissionlessly).
    pub pending_withdraw_slot_le: [u8; 8],
}

assert_no_padding!(Vault, 32 * 2 + 8 + 1 + 1 + 32 + 16 + 8 + 8);

impl Discriminator for Vault {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::VaultDiscriminator as u8;
}

impl Versioned for Vault {
    // v2: the dead `maintenance_margin_bps`/`liquidation_penalty_bps` duplicates
    // were removed (known-issues §3); bump so a pre-v2 vault fails the version
    // check loudly rather than mis-reading `authority_bump`/`bump`.
    // v3 (plan.md §3.4): appended `authority` (the withdraw admin),
    // `total_user_balance` (the on-chain backing aggregate for the fail-closed
    // outflow gate, missing-features §4.2), and the staged insurance-withdraw
    // slot. The PDA seeds are per-mint, so a pre-v3 vault cannot be re-created
    // at the same address — re-provision means a FRESH collateral mint.
    const VERSION: u8 = 3;
}

impl AccountSize for Vault {
    const DATA_LEN: usize = 32 * 2 + 8 + 1 + 1 + 32 + 16 + 8 + 8;
}

impl AccountDeserialize for Vault {}

impl AccountSerialize for Vault {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.collateral_mint.as_ref());
        data.extend_from_slice(self.vault_token_account.as_ref());
        data.extend_from_slice(&self.insurance_balance_le);
        data.push(self.authority_bump);
        data.push(self.bump);
        data.extend_from_slice(self.authority.as_ref());
        data.extend_from_slice(&self.total_user_balance_le);
        data.extend_from_slice(&self.pending_withdraw_amount_le);
        data.extend_from_slice(&self.pending_withdraw_slot_le);
        data
    }
}

impl PdaSeeds for Vault {
    const PREFIX: &'static [u8] = b"vault";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.collateral_mint.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.collateral_mint.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for Vault {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl Vault {
    le_field!(
        insurance_balance,
        set_insurance_balance,
        insurance_balance_le,
        u64
    );
    le_field!(
        total_user_balance,
        set_total_user_balance,
        total_user_balance_le,
        u128
    );
    le_field!(
        pending_withdraw_amount,
        set_pending_withdraw_amount,
        pending_withdraw_amount_le,
        u64
    );
    le_field!(
        pending_withdraw_slot,
        set_pending_withdraw_slot,
        pending_withdraw_slot_le,
        u64
    );

    /// Vault-authority PDA seed (owns the token account, signs withdrawals).
    pub const AUTHORITY_PREFIX: &'static [u8] = b"vault_authority";

    #[inline(always)]
    pub fn new(
        bump: u8,
        authority_bump: u8,
        collateral_mint: Address,
        vault_token_account: Address,
        authority: Address,
    ) -> Self {
        Self {
            collateral_mint,
            vault_token_account,
            insurance_balance_le: 0u64.to_le_bytes(),
            authority_bump,
            bump,
            authority,
            total_user_balance_le: 0u128.to_le_bytes(),
            pending_withdraw_amount_le: 0u64.to_le_bytes(),
            pending_withdraw_slot_le: 0u64.to_le_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    #[test]
    fn test_vault_roundtrip() {
        let v = Vault::new(
            255,
            254,
            Address::new_from_array([1u8; 32]),
            Address::new_from_array([2u8; 32]),
            Address::new_from_array([3u8; 32]),
        );
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), Vault::LEN);
        assert_eq!(bytes[0], Vault::DISCRIMINATOR);
        assert_eq!(bytes[1], Vault::VERSION);
        assert_eq!(Vault::VERSION, 3);
        let de = Vault::from_bytes(&bytes).unwrap();
        assert_eq!(de.collateral_mint, v.collateral_mint);
        assert_eq!(de.vault_token_account, v.vault_token_account);
        assert_eq!(de.authority_bump, 254);
        assert_eq!(de.bump, 255);
        assert_eq!(de.insurance_balance(), 0);
        // v3 fields round-trip; a fresh vault has no user claims, no pending withdraw.
        assert_eq!(de.authority, Address::new_from_array([3u8; 32]));
        assert_eq!(de.total_user_balance(), 0);
        assert_eq!(de.pending_withdraw_amount(), 0);
        assert_eq!(de.pending_withdraw_slot(), 0);
    }

    #[test]
    fn test_insurance_balance_setter() {
        let mut v = Vault::new(
            1,
            2,
            Address::new_from_array([0u8; 32]),
            Address::new_from_array([0u8; 32]),
            Address::new_from_array([9u8; 32]),
        );
        v.set_insurance_balance(1_000_000);
        assert_eq!(v.insurance_balance(), 1_000_000);
        v.set_total_user_balance(42);
        assert_eq!(v.total_user_balance(), 42);
    }
}
