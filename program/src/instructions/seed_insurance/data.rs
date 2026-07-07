use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for SeedInsurance.
///
/// # Layout (little-endian)
/// * `amount` (u64) — tokens to donate into the insurance pool (must be non-zero).
pub struct SeedInsuranceData {
    pub amount: u64,
}

impl<'a> TryFrom<&'a [u8]> for SeedInsuranceData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        let amount = u64::from_le_bytes(data[0..8].try_into().unwrap());
        if amount == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self { amount })
    }
}

impl<'a> InstructionData<'a> for SeedInsuranceData {
    const LEN: usize = 8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let d = SeedInsuranceData::try_from(&7u64.to_le_bytes()[..]).unwrap();
        assert_eq!(d.amount, 7);
    }

    #[test]
    fn test_zero_rejected() {
        assert!(SeedInsuranceData::try_from(&0u64.to_le_bytes()[..]).is_err());
    }

    #[test]
    fn test_wrong_len_rejected() {
        assert!(SeedInsuranceData::try_from(&[0u8; 4][..]).is_err());
    }
}
