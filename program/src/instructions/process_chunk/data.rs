use pinocchio::error::ProgramError;

use crate::{errors::TempoProgramError, require_len, traits::InstructionData};

/// Instruction data for ProcessChunk.
///
/// # Layout (little-endian)
/// * `start_index` (u32) — first slab slot to process
/// * `max_count` (u32) — max slots to process this chunk (bounds CU)
pub struct ProcessChunkData {
    pub start_index: u32,
    pub max_count: u32,
}

impl<'a> TryFrom<&'a [u8]> for ProcessChunkData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        let start_index = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let max_count = u32::from_le_bytes(data[4..8].try_into().unwrap());
        if max_count == 0 {
            return Err(TempoProgramError::InvalidQuantity.into());
        }
        Ok(Self {
            start_index,
            max_count,
        })
    }
}

impl<'a> InstructionData<'a> for ProcessChunkData {
    const LEN: usize = 4 + 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&5u32.to_le_bytes());
        buf[4..8].copy_from_slice(&16u32.to_le_bytes());
        let d = ProcessChunkData::try_from(&buf[..]).unwrap();
        assert_eq!(d.start_index, 5);
        assert_eq!(d.max_count, 16);
    }

    #[test]
    fn test_zero_max_count_rejected() {
        let buf = [0u8; 8];
        assert_eq!(
            ProcessChunkData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::InvalidQuantity.into()
        );
    }
}
