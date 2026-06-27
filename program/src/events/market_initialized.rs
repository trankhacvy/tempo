use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct MarketInitializedEvent {
    pub market: Address,
    pub authority: Address,
    pub tick_size: u64,
    pub num_ticks: u32,
    pub orders_per_auction_cap: u32,
}

impl EventDiscriminator for MarketInitializedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::MarketInitialized as u8;
}

impl EventSerialize for MarketInitializedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.authority.as_ref());
        data.extend_from_slice(&self.tick_size.to_le_bytes());
        data.extend_from_slice(&self.num_ticks.to_le_bytes());
        data.extend_from_slice(&self.orders_per_auction_cap.to_le_bytes());
        data
    }
}

impl MarketInitializedEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 4 + 4;
}
