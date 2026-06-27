use alloc::vec;
use alloc::vec::Vec;
use codama::CodamaAccount;
use pinocchio::{cpi::Seed, Address};

use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};
use crate::{assert_no_padding, le_field};

/// `ClearingResult` — the small, fixed-size output of Phase 2 DISCOVER
/// (clearing-protocol §3, system-design §6.2).
///
/// Every order self-computes its own fill in Phase 3 from these constants with
/// zero cross-order coordination. Both the bid auction (maker-buys vs
/// taker-sells) and the ask auction (taker-buys vs maker-sells) are published;
/// each order reads the side matching its `(side, is_maker)` role.
///
/// # Marginal-tick rationing
/// Orders strictly better than the marginal tick fill fully. Orders *at* the
/// marginal tick on the rationed side fill pro-rata:
/// `fill = order_qty * volume_allocated_to_marginal_tick / total_qty_at_marginal_tick`
/// with floor division (rounds against the filler; dust stays with the protocol).
///
/// # PDA Seeds
/// `[b"clearing", market.as_ref()]` (one persistent result account per market,
/// reused and overwritten each round).
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1** — see `le_field!`)
/// 9 × [u8;8] (72) + 2 × [u8;4] (8) + Address (32) + u8 (1) + 2 × u8 (2) = 115.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 3))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "clearing"))]
#[codama(seed(name = "market", type = public_key))]
#[repr(C)]
pub struct ClearingResult {
    pub auction_id_le: [u8; 8],
    pub bid_clearing_price_le: [u8; 8],
    pub ask_clearing_price_le: [u8; 8],
    pub bid_matched_volume_le: [u8; 8],
    pub ask_matched_volume_le: [u8; 8],
    pub bid_volume_allocated_to_marginal_tick_le: [u8; 8],
    pub bid_total_qty_at_marginal_tick_le: [u8; 8],
    pub ask_volume_allocated_to_marginal_tick_le: [u8; 8],
    pub ask_total_qty_at_marginal_tick_le: [u8; 8],
    pub bid_marginal_tick_le: [u8; 4],
    pub ask_marginal_tick_le: [u8; 4],
    /// Market this result belongs to (part of the PDA seeds).
    pub market: Address,
    /// PDA bump.
    pub bump: u8,
    /// Which side the bid auction rationed (`clearing::RATIONED_*`).
    pub bid_rationed_side: u8,
    /// Which side the ask auction rationed (`clearing::RATIONED_*`).
    pub ask_rationed_side: u8,
}

assert_no_padding!(ClearingResult, 8 * 9 + 4 * 2 + 32 + 1 + 2);

impl Discriminator for ClearingResult {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::ClearingResultDiscriminator as u8;
}

impl Versioned for ClearingResult {
    const VERSION: u8 = 1;
}

impl AccountSize for ClearingResult {
    const DATA_LEN: usize = 8 * 9 + 4 * 2 + 32 + 1 + 2;
}

impl AccountDeserialize for ClearingResult {}

impl AccountSerialize for ClearingResult {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(&self.auction_id_le);
        data.extend_from_slice(&self.bid_clearing_price_le);
        data.extend_from_slice(&self.ask_clearing_price_le);
        data.extend_from_slice(&self.bid_matched_volume_le);
        data.extend_from_slice(&self.ask_matched_volume_le);
        data.extend_from_slice(&self.bid_volume_allocated_to_marginal_tick_le);
        data.extend_from_slice(&self.bid_total_qty_at_marginal_tick_le);
        data.extend_from_slice(&self.ask_volume_allocated_to_marginal_tick_le);
        data.extend_from_slice(&self.ask_total_qty_at_marginal_tick_le);
        data.extend_from_slice(&self.bid_marginal_tick_le);
        data.extend_from_slice(&self.ask_marginal_tick_le);
        data.extend_from_slice(self.market.as_ref());
        data.push(self.bump);
        data.push(self.bid_rationed_side);
        data.push(self.ask_rationed_side);
        data
    }
}

impl PdaSeeds for ClearingResult {
    const PREFIX: &'static [u8] = b"clearing";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.market.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for ClearingResult {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl ClearingResult {
    le_field!(auction_id, set_auction_id, auction_id_le, u64);
    le_field!(
        bid_clearing_price,
        set_bid_clearing_price,
        bid_clearing_price_le,
        u64
    );
    le_field!(
        ask_clearing_price,
        set_ask_clearing_price,
        ask_clearing_price_le,
        u64
    );
    le_field!(
        bid_matched_volume,
        set_bid_matched_volume,
        bid_matched_volume_le,
        u64
    );
    le_field!(
        ask_matched_volume,
        set_ask_matched_volume,
        ask_matched_volume_le,
        u64
    );
    le_field!(
        bid_volume_allocated_to_marginal_tick,
        set_bid_volume_allocated_to_marginal_tick,
        bid_volume_allocated_to_marginal_tick_le,
        u64
    );
    le_field!(
        bid_total_qty_at_marginal_tick,
        set_bid_total_qty_at_marginal_tick,
        bid_total_qty_at_marginal_tick_le,
        u64
    );
    le_field!(
        ask_volume_allocated_to_marginal_tick,
        set_ask_volume_allocated_to_marginal_tick,
        ask_volume_allocated_to_marginal_tick_le,
        u64
    );
    le_field!(
        ask_total_qty_at_marginal_tick,
        set_ask_total_qty_at_marginal_tick,
        ask_total_qty_at_marginal_tick_le,
        u64
    );
    le_field!(
        bid_marginal_tick,
        set_bid_marginal_tick,
        bid_marginal_tick_le,
        u32
    );
    le_field!(
        ask_marginal_tick,
        set_ask_marginal_tick,
        ask_marginal_tick_le,
        u32
    );

    #[inline(always)]
    pub fn empty(bump: u8, market: Address, auction_id: u64) -> Self {
        Self {
            auction_id_le: auction_id.to_le_bytes(),
            bid_clearing_price_le: [0u8; 8],
            ask_clearing_price_le: [0u8; 8],
            bid_matched_volume_le: [0u8; 8],
            ask_matched_volume_le: [0u8; 8],
            bid_volume_allocated_to_marginal_tick_le: [0u8; 8],
            bid_total_qty_at_marginal_tick_le: [0u8; 8],
            ask_volume_allocated_to_marginal_tick_le: [0u8; 8],
            ask_total_qty_at_marginal_tick_le: [0u8; 8],
            bid_marginal_tick_le: [0u8; 4],
            ask_marginal_tick_le: [0u8; 4],
            market,
            bump,
            bid_rationed_side: crate::clearing::RATIONED_NONE,
            ask_rationed_side: crate::clearing::RATIONED_NONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    fn sample() -> ClearingResult {
        let mut r = ClearingResult::empty(255, Address::new_from_array([8u8; 32]), 42);
        r.set_bid_clearing_price(120);
        r.set_bid_matched_volume(1000);
        r.set_bid_marginal_tick(11);
        r.set_bid_volume_allocated_to_marginal_tick(250);
        r.set_bid_total_qty_at_marginal_tick(400);
        r.set_ask_clearing_price(130);
        r.set_ask_matched_volume(500);
        r.set_ask_marginal_tick(13);
        r.bid_rationed_side = crate::clearing::RATIONED_DEMAND;
        r.ask_rationed_side = crate::clearing::RATIONED_SUPPLY;
        r
    }

    #[test]
    fn test_clearing_result_roundtrip() {
        let r = sample();
        let bytes = r.to_bytes();
        assert_eq!(bytes.len(), ClearingResult::LEN);
        assert_eq!(bytes[0], ClearingResult::DISCRIMINATOR);
        assert_eq!(bytes[1], ClearingResult::VERSION);

        let de = ClearingResult::from_bytes(&bytes).unwrap();
        assert_eq!(de.auction_id(), 42);
        assert_eq!(de.bid_clearing_price(), 120);
        assert_eq!(de.bid_matched_volume(), 1000);
        assert_eq!(de.bid_marginal_tick(), 11);
        assert_eq!(de.bid_volume_allocated_to_marginal_tick(), 250);
        assert_eq!(de.bid_total_qty_at_marginal_tick(), 400);
        assert_eq!(de.ask_clearing_price(), 130);
        assert_eq!(de.ask_matched_volume(), 500);
        assert_eq!(de.ask_marginal_tick(), 13);
        assert_eq!(de.market, r.market);
        assert_eq!(de.bump, 255);
        assert_eq!(de.bid_rationed_side, crate::clearing::RATIONED_DEMAND);
        assert_eq!(de.ask_rationed_side, crate::clearing::RATIONED_SUPPLY);
    }

    #[test]
    fn test_clearing_result_empty_defaults() {
        let r = ClearingResult::empty(1, Address::new_from_array([3u8; 32]), 7);
        assert_eq!(r.auction_id(), 7);
        assert_eq!(r.bid_clearing_price(), 0);
        assert_eq!(r.ask_matched_volume(), 0);
    }
}
