use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `apply_insurance_withdraw` (plan.md §4.4): the staged,
/// delay-gated, backing-checked authority withdrawal from the insurance pool.
#[derive(CodamaType)]
pub struct InsuranceWithdrawnEvent {
    pub collateral_mint: Address,
    pub authority: Address,
    pub amount: u64,
}

impl EventDiscriminator for InsuranceWithdrawnEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::InsuranceWithdrawn as u8;
}

impl EventSerialize for InsuranceWithdrawnEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.collateral_mint.as_ref());
        data.extend_from_slice(self.authority.as_ref());
        data.extend_from_slice(&self.amount.to_le_bytes());
        data
    }
}

impl InsuranceWithdrawnEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8;
}
