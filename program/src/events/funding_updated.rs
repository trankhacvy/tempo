use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `update_funding` after the market's funding index advances.
#[derive(CodamaType)]
pub struct FundingUpdatedEvent {
    pub market: Address,
    pub funding_index: i128,
    pub mark: u64,
    pub oracle_price_1e8: u64,
}

impl EventDiscriminator for FundingUpdatedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::FundingUpdated as u8;
}

impl EventSerialize for FundingUpdatedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(&self.funding_index.to_le_bytes());
        data.extend_from_slice(&self.mark.to_le_bytes());
        data.extend_from_slice(&self.oracle_price_1e8.to_le_bytes());
        data
    }
}

impl FundingUpdatedEvent {
    pub const DATA_LEN: usize = 32 + 16 + 8 + 8;
}
