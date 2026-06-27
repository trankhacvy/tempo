use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct ClearingFinalizedEvent {
    pub market: Address,
    pub auction_id: u64,
    pub bid_clearing_price: u64,
    pub bid_matched_volume: u64,
    pub ask_clearing_price: u64,
    pub ask_matched_volume: u64,
}

impl EventDiscriminator for ClearingFinalizedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::ClearingFinalized as u8;
}

impl EventSerialize for ClearingFinalizedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(&self.auction_id.to_le_bytes());
        data.extend_from_slice(&self.bid_clearing_price.to_le_bytes());
        data.extend_from_slice(&self.bid_matched_volume.to_le_bytes());
        data.extend_from_slice(&self.ask_clearing_price.to_le_bytes());
        data.extend_from_slice(&self.ask_matched_volume.to_le_bytes());
        data
    }
}

impl ClearingFinalizedEvent {
    pub const DATA_LEN: usize = 32 + 8 + 8 + 8 + 8 + 8;
}
