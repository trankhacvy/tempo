use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct OrderCancelledEvent {
    pub market: Address,
    pub trader: Address,
    pub order_id: u64,
    pub auction_id: u64,
}

impl EventDiscriminator for OrderCancelledEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::OrderCancelled as u8;
}

impl EventSerialize for OrderCancelledEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.trader.as_ref());
        data.extend_from_slice(&self.order_id.to_le_bytes());
        data.extend_from_slice(&self.auction_id.to_le_bytes());
        data
    }
}

impl OrderCancelledEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 8;
}
