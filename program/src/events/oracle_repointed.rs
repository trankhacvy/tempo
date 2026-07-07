use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `apply_set_oracle` (plan.md §3.3): the market's price feed was
/// repointed (staged + delayed + quiescence-gated). Indexers should treat this
/// as a price-regime boundary.
#[derive(CodamaType)]
pub struct OracleRepointedEvent {
    pub market: Address,
    pub old_oracle: Address,
    pub new_oracle: Address,
    pub new_feed_id: [u8; 32],
}

impl EventDiscriminator for OracleRepointedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::OracleRepointed as u8;
}

impl EventSerialize for OracleRepointedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.old_oracle.as_ref());
        data.extend_from_slice(self.new_oracle.as_ref());
        data.extend_from_slice(&self.new_feed_id);
        data
    }
}

impl OracleRepointedEvent {
    pub const DATA_LEN: usize = 32 * 4;
}
