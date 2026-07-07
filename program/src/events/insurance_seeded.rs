use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `seed_insurance` (missing-features §4.1): a permissionless donation
/// into the vault's insurance pool. Both sides of the backing invariant
/// (`vault_token ≥ Σ balances + insurance`) grow together, so this can never
/// mint money — it exists so a fresh market's pool is not zero (the P0.6 devnet
/// drill deadlocked on exactly that: the first profitable maker settle failed
/// `InsuranceInsolvent` forever on an empty pool).
#[derive(CodamaType)]
pub struct InsuranceSeededEvent {
    pub collateral_mint: Address,
    pub donor: Address,
    pub amount: u64,
}

impl EventDiscriminator for InsuranceSeededEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::InsuranceSeeded as u8;
}

impl EventSerialize for InsuranceSeededEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.collateral_mint.as_ref());
        data.extend_from_slice(self.donor.as_ref());
        data.extend_from_slice(&self.amount.to_le_bytes());
        data
    }
}

impl InsuranceSeededEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8;
}
