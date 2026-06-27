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
/// 2 × Address (64) + [u8;8] (8) + 2 × u8 (2) = 74.
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
}

assert_no_padding!(Vault, 32 * 2 + 8 + 1 + 1);

impl Discriminator for Vault {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::VaultDiscriminator as u8;
}

impl Versioned for Vault {
    // v2: the dead `maintenance_margin_bps`/`liquidation_penalty_bps` duplicates
    // were removed (known-issues §3); bump so a pre-v2 vault fails the version
    // check loudly rather than mis-reading `authority_bump`/`bump`.
    const VERSION: u8 = 2;
}

impl AccountSize for Vault {
    const DATA_LEN: usize = 32 * 2 + 8 + 1 + 1;
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

    /// Vault-authority PDA seed (owns the token account, signs withdrawals).
    pub const AUTHORITY_PREFIX: &'static [u8] = b"vault_authority";

    #[inline(always)]
    pub fn new(
        bump: u8,
        authority_bump: u8,
        collateral_mint: Address,
        vault_token_account: Address,
    ) -> Self {
        Self {
            collateral_mint,
            vault_token_account,
            insurance_balance_le: 0u64.to_le_bytes(),
            authority_bump,
            bump,
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
        );
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), Vault::LEN);
        assert_eq!(bytes[0], Vault::DISCRIMINATOR);
        let de = Vault::from_bytes(&bytes).unwrap();
        assert_eq!(de.collateral_mint, v.collateral_mint);
        assert_eq!(de.vault_token_account, v.vault_token_account);
        assert_eq!(de.authority_bump, 254);
        assert_eq!(de.bump, 255);
        assert_eq!(de.insurance_balance(), 0);
    }

    #[test]
    fn test_insurance_balance_setter() {
        let mut v = Vault::new(
            1,
            2,
            Address::new_from_array([0u8; 32]),
            Address::new_from_array([0u8; 32]),
        );
        v.set_insurance_balance(1_000_000);
        assert_eq!(v.insurance_balance(), 1_000_000);
    }
}
