use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct FillSettledEvent {
    pub market: Address,
    pub trader: Address,
    pub order_id: u64,
    pub auction_id: u64,
    pub fill: u64,
    pub side: u8,
    pub is_maker: u8,
    /// Stage A sharding: which OrderSlab shard the settled order lived in.
    pub shard_id: u16,
}

impl EventDiscriminator for FillSettledEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::FillSettled as u8;
}

impl EventSerialize for FillSettledEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.trader.as_ref());
        data.extend_from_slice(&self.order_id.to_le_bytes());
        data.extend_from_slice(&self.auction_id.to_le_bytes());
        data.extend_from_slice(&self.fill.to_le_bytes());
        data.push(self.side);
        data.push(self.is_maker);
        data.extend_from_slice(&self.shard_id.to_le_bytes());
        data
    }
}

impl FillSettledEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 8 + 8 + 1 + 1 + 2;
}
