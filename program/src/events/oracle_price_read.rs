use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct OraclePriceReadEvent {
    pub market: Address,
    pub oracle_price_1e8: u64,
    pub exponent: i32,
    pub publish_time: i64,
    pub mark_price: u64,
}

impl EventDiscriminator for OraclePriceReadEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::OraclePriceRead as u8;
}

impl EventSerialize for OraclePriceReadEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(&self.oracle_price_1e8.to_le_bytes());
        data.extend_from_slice(&self.exponent.to_le_bytes());
        data.extend_from_slice(&self.publish_time.to_le_bytes());
        data.extend_from_slice(&self.mark_price.to_le_bytes());
        data
    }
}

impl OraclePriceReadEvent {
    pub const DATA_LEN: usize = 32 + 8 + 4 + 8 + 8;
}
