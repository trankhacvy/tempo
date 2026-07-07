use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ProposeInsuranceWithdraw (plan.md §4.4): the
/// authority-controlled token OUTFLOW is staged behind the delay — a
/// compromised authority draining insurance instantly is the priced-in
/// scenario; users get one window to exit.
///
/// # Layout
/// * `amount` (u64) — tokens to withdraw from the pool (must be non-zero)
pub struct ProposeInsuranceWithdrawData {
    pub amount: u64,
}

impl<'a> TryFrom<&'a [u8]> for ProposeInsuranceWithdrawData {
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

impl<'a> InstructionData<'a> for ProposeInsuranceWithdrawData {
    const LEN: usize = 8;
}
