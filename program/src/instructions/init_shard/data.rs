use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitShard (Stage A sharding).
///
/// # Layout (little-endian)
/// * `shard_id` (u16) — index of the shard to create (`[0, num_slab_shards)`)
/// * `bump` (u8) — bump for the shard's OrderSlab PDA
pub struct InitShardData {
    pub shard_id: u16,
    pub bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for InitShardData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        let shard_id = u16::from_le_bytes(data[0..2].try_into().unwrap());
        let bump = data[2];
        Ok(Self { shard_id, bump })
    }
}

impl<'a> InstructionData<'a> for InitShardData {
    const LEN: usize = 2 + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let mut buf = [0u8; 3];
        buf[0..2].copy_from_slice(&7u16.to_le_bytes());
        buf[2] = 254;
        let d = InitShardData::try_from(&buf[..]).unwrap();
        assert_eq!(d.shard_id, 7);
        assert_eq!(d.bump, 254);
    }

    #[test]
    fn test_too_short() {
        assert!(matches!(
            InitShardData::try_from(&[0u8; 2][..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
