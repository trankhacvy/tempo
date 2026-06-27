use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct ChunkProcessedEvent {
    pub market: Address,
    pub cranker: Address,
    pub auction_id: u64,
    pub folded: u64,
    pub accumulated_total: u64,
}

impl EventDiscriminator for ChunkProcessedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::ChunkProcessed as u8;
}

impl EventSerialize for ChunkProcessedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.cranker.as_ref());
        data.extend_from_slice(&self.auction_id.to_le_bytes());
        data.extend_from_slice(&self.folded.to_le_bytes());
        data.extend_from_slice(&self.accumulated_total.to_le_bytes());
        data
    }
}

impl ChunkProcessedEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 8 + 8;
}
