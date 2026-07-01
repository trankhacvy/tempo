use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

#[derive(CodamaType)]
pub struct OrderSubmittedEvent {
    pub market: Address,
    pub trader: Address,
    pub order_id: u64,
    pub auction_id: u64,
    pub price: u64,
    pub quantity: u64,
    /// Slab slot index this order was written to. Clients pass it back as the
    /// O(1) `slot_hint` on `cancel_order`/`settle_fill` (known-issues §2.7); the
    /// program validates it against the slot's `order_id` and falls back to a scan
    /// if it is stale, so it is an optimization hint, never a trust input.
    pub slot: u32,
    pub side: u8,
    pub is_maker: u8,
    /// Stage A sharding: which OrderSlab shard the order was written to. Clients pass
    /// this shard's PDA as the `order_slab` account on `cancel_order`/`settle_fill`.
    pub shard_id: u16,
}

impl EventDiscriminator for OrderSubmittedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::OrderSubmitted as u8;
}

impl EventSerialize for OrderSubmittedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.trader.as_ref());
        data.extend_from_slice(&self.order_id.to_le_bytes());
        data.extend_from_slice(&self.auction_id.to_le_bytes());
        data.extend_from_slice(&self.price.to_le_bytes());
        data.extend_from_slice(&self.quantity.to_le_bytes());
        data.extend_from_slice(&self.slot.to_le_bytes());
        data.push(self.side);
        data.push(self.is_maker);
        data.extend_from_slice(&self.shard_id.to_le_bytes());
        data
    }
}

impl OrderSubmittedEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 8 + 8 + 8 + 4 + 1 + 1 + 2;
}
