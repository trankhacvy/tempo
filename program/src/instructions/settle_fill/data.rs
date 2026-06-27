use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for SettleFill.
///
/// # Layout (little-endian)
/// * `order_id` (u64) — the single order to settle
/// * `slot_hint` (u32) — the slab slot index `order_id` is expected at (from the
///   `OrderSubmitted` event). An O(1) optimization: the program checks this slot
///   first and only scans the slab if the hint is stale, so a wrong hint is never
///   a trust input (known-issues §2.7).
pub struct SettleFillData {
    pub order_id: u64,
    pub slot_hint: u32,
}

impl<'a> TryFrom<&'a [u8]> for SettleFillData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            order_id: u64::from_le_bytes(data[0..8].try_into().unwrap()),
            slot_hint: u32::from_le_bytes(data[8..12].try_into().unwrap()),
        })
    }
}

impl<'a> InstructionData<'a> for SettleFillData {
    const LEN: usize = 12;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let mut buf = [0u8; 12];
        buf[0..8].copy_from_slice(&9u64.to_le_bytes());
        buf[8..12].copy_from_slice(&3u32.to_le_bytes());
        let d = SettleFillData::try_from(&buf[..]).unwrap();
        assert_eq!(d.order_id, 9);
        assert_eq!(d.slot_hint, 3);
    }

    #[test]
    fn test_too_short() {
        let buf = [0u8; 9];
        assert!(matches!(
            SettleFillData::try_from(&buf[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
