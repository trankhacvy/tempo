use pinocchio::error::ProgramError;

use crate::{errors::TempoProgramError, state::OrderSide, traits::InstructionData};

/// Instruction data for SubmitOrder.
///
/// `submit_order` is **taker-only** (known-issues §1.3 / Option A): a trader can
/// no longer self-declare `is_maker`, since that single client byte used to steer
/// which uniform cross the order cleared in *and* which fee tier it paid. Maker
/// liquidity now comes exclusively from the on-chain `MakerQuote` book
/// (`init_maker_quote` → `process_maker_quote`), where "maker" is a verifiable fact
/// rather than a self-asserted flag.
///
/// # Layout (little-endian)
/// * `side` (u8) — 0 buy, 1 sell
/// * `price` (u64)
/// * `quantity` (u64)
/// * `reduce_only` (u8) — 1 = the order is intended only to *reduce* an existing
///   opposite position. It reserves the same FULL worst-case margin as a normal
///   order (DDR-3 Correction-2 item 3 — no headroom discount; see the processor);
///   its sole effect is forcing `Consumed` at settle (never re-armed `Resting`),
///   so it can never carry across rounds (missing-features §2.2). 0 = normal.
/// * `shard_id` (u16) — which `OrderSlab` shard to insert into (`[0, num_slab_shards)`).
///   The client picks the shard (least-full / hash) and passes the resolved shard PDA
///   as the `order_slab` account; the processor validates the PDA against this index.
/// * `expires_at_auction` (u64) — Stage B resting-order expiry. `0` = good-till-cancelled
///   (the order rests until filled or cancelled). Otherwise an absolute auction id: the
///   order stops resting once `expires_at_auction <= current_auction_id` (its leftover is
///   `Consumed` at that round's settle instead of re-armed). A client sets e.g.
///   `current_auction_id + 20` to bound how long the order squats a slab slot, or sets
///   it EQUAL to the arm round (the current auction id in `Collect`, `current + 1`
///   mid-round) for an IOC order: it participates in exactly one auction and any
///   unfilled remainder is consumed there (missing-features §2.3). An expiry strictly
///   before the arm round is rejected (`OrderAlreadyExpired`).
pub struct SubmitOrderData {
    pub side: u8,
    pub price: u64,
    pub quantity: u64,
    pub reduce_only: bool,
    pub shard_id: u16,
    pub expires_at_auction: u64,
}

impl<'a> TryFrom<&'a [u8]> for SubmitOrderData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        // EXACT length, not a minimum: the wire format is fixed-size. The body is 28
        // bytes (`reduce_only` + `shard_id` (Stage A sharding) + `expires_at_auction`
        // (Stage B resting orders)). Any other length fails loud ("invalid instruction
        // data") rather than mis-parsing a shifted price/quantity.
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }

        let side = data[0];
        let price = u64::from_le_bytes(data[1..9].try_into().unwrap());
        let quantity = u64::from_le_bytes(data[9..17].try_into().unwrap());
        let reduce_only = data[17] != 0;
        let shard_id = u16::from_le_bytes(data[18..20].try_into().unwrap());
        let expires_at_auction = u64::from_le_bytes(data[20..28].try_into().unwrap());

        // Validate the side byte; price tick-alignment is validated against the
        // market in the processor (needs tick_size).
        OrderSide::from_u8(side)?;
        if quantity == 0 {
            return Err(TempoProgramError::ZeroQuantity.into());
        }

        Ok(Self {
            side,
            price,
            quantity,
            reduce_only,
            shard_id,
            expires_at_auction,
        })
    }
}

impl<'a> InstructionData<'a> for SubmitOrderData {
    const LEN: usize = 1 + 8 + 8 + 1 + 2 + 8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(side: u8, price: u64, qty: u64) -> [u8; 28] {
        let mut buf = [0u8; 28];
        buf[0] = side;
        buf[1..9].copy_from_slice(&price.to_le_bytes());
        buf[9..17].copy_from_slice(&qty.to_le_bytes());
        // buf[17] = reduce_only, defaults to 0 (normal order)
        // buf[18..20] = shard_id, defaults to 0
        // buf[20..28] = expires_at_auction, defaults to 0 (GTC)
        buf
    }

    #[test]
    fn test_valid() {
        let buf = encode(1, 100, 50);
        let d = SubmitOrderData::try_from(&buf[..]).unwrap();
        assert_eq!(d.side, 1);
        assert_eq!(d.price, 100);
        assert_eq!(d.quantity, 50);
        assert!(!d.reduce_only);
        assert_eq!(d.shard_id, 0);
        assert_eq!(d.expires_at_auction, 0);
    }

    #[test]
    fn test_shard_id_parsed() {
        let mut buf = encode(0, 100, 50);
        buf[18..20].copy_from_slice(&7u16.to_le_bytes());
        let d = SubmitOrderData::try_from(&buf[..]).unwrap();
        assert_eq!(d.shard_id, 7);
    }

    #[test]
    fn test_expires_at_auction_parsed() {
        let mut buf = encode(0, 100, 50);
        buf[20..28].copy_from_slice(&123u64.to_le_bytes());
        let d = SubmitOrderData::try_from(&buf[..]).unwrap();
        assert_eq!(d.expires_at_auction, 123);
    }

    #[test]
    fn test_reduce_only_flag_parsed() {
        let mut buf = encode(0, 100, 50);
        buf[17] = 1;
        let d = SubmitOrderData::try_from(&buf[..]).unwrap();
        assert!(d.reduce_only);
    }

    #[test]
    fn test_zero_qty_rejected() {
        let buf = encode(0, 100, 0);
        assert_eq!(
            SubmitOrderData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::ZeroQuantity.into()
        );
    }

    #[test]
    fn test_bad_side_rejected() {
        let buf = encode(2, 100, 5);
        assert_eq!(
            SubmitOrderData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::InvalidOrderSide.into()
        );
    }

    #[test]
    fn test_old_20_byte_body_rejected() {
        // The Stage A body was 20 bytes (no `expires_at_auction`). The exact-length
        // gate now requires 28, so a stale client fails loud rather than submitting
        // with an unspecified expiry.
        let buf = [0u8; 20];
        assert!(matches!(
            SubmitOrderData::try_from(&buf[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }

    #[test]
    fn test_too_short() {
        let buf = [0u8; 4];
        assert!(matches!(
            SubmitOrderData::try_from(&buf[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
