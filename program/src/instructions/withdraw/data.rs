use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for Withdraw.
///
/// # Layout
/// * `amount` (u64 LE) — collateral base units to withdraw (must be > 0)
pub struct WithdrawData {
    pub amount: u64,
}

impl<'a> TryFrom<&'a [u8]> for WithdrawData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        let amount = u64::from_le_bytes(
            data[0..8]
                .try_into()
                .map_err(|_| ProgramError::InvalidInstructionData)?,
        );
        if amount == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self { amount })
    }
}

impl<'a> InstructionData<'a> for WithdrawData {
    const LEN: usize = 8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        assert_eq!(
            WithdrawData::try_from(&500u64.to_le_bytes()[..])
                .unwrap()
                .amount,
            500
        );
    }

    #[test]
    fn test_zero_rejected() {
        assert!(matches!(
            WithdrawData::try_from(&0u64.to_le_bytes()[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }

    #[test]
    fn test_too_short() {
        assert!(matches!(
            WithdrawData::try_from(&[0u8; 4][..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
