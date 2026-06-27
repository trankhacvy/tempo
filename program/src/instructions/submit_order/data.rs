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
/// * `reduce_only` (u8) — 1 = the order may only *reduce* an existing opposite
///   position; the processor reserves margin only for any portion that would open
///   new exposure, so a close is not blocked by the worst-case reservation
///   (missing-features §1.1/§2.2). 0 = a normal order (reserves the full worst case).
pub struct SubmitOrderData {
    pub side: u8,
    pub price: u64,
    pub quantity: u64,
    pub reduce_only: bool,
}

impl<'a> TryFrom<&'a [u8]> for SubmitOrderData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        // EXACT length, not a minimum: the wire format is fixed-size. The body is 18
        // bytes (the trailing `reduce_only` flag, missing-features §1.1). Any other
        // length fails loud ("invalid instruction data") rather than mis-parsing a
        // shifted price/quantity.
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }

        let side = data[0];
        let price = u64::from_le_bytes(data[1..9].try_into().unwrap());
        let quantity = u64::from_le_bytes(data[9..17].try_into().unwrap());
        let reduce_only = data[17] != 0;

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
        })
    }
}

impl<'a> InstructionData<'a> for SubmitOrderData {
    const LEN: usize = 1 + 8 + 8 + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(side: u8, price: u64, qty: u64) -> [u8; 18] {
        let mut buf = [0u8; 18];
        buf[0] = side;
        buf[1..9].copy_from_slice(&price.to_le_bytes());
        buf[9..17].copy_from_slice(&qty.to_le_bytes());
        // buf[17] = reduce_only, defaults to 0 (normal order)
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
    fn test_old_17_byte_body_rejected() {
        // The pre-reservation body was 17 bytes (no reduce_only). The exact-length
        // gate now requires 18, so a stale 17-byte client fails loud rather than
        // submitting with an unspecified reduce_only.
        let buf = [0u8; 17];
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
