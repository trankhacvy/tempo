use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ProposeRiskUpdate (plan.md §3.2, the STAGED risk set —
/// raising maintenance can make live positions liquidatable, so it sits behind
/// the propose→delay→apply engine; users get the delay window to de-risk).
///
/// # Layout (little-endian, 8 bytes — exactly the staged payload)
/// * `maintenance_margin_bps` (u16) · `initial_margin_bps` (u16)
/// * `liquidation_penalty_bps` (u16) · `liquidation_close_buffer_bps` (u16)
pub struct ProposeRiskUpdateData {
    pub maintenance_margin_bps: u16,
    pub initial_margin_bps: u16,
    pub liquidation_penalty_bps: u16,
    pub liquidation_close_buffer_bps: u16,
}

impl<'a> TryFrom<&'a [u8]> for ProposeRiskUpdateData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        // Bounds are validated in the processor via the SHARED
        // `validate_risk_config` (one source of truth with initialize_market).
        Ok(Self {
            maintenance_margin_bps: u16::from_le_bytes(data[0..2].try_into().unwrap()),
            initial_margin_bps: u16::from_le_bytes(data[2..4].try_into().unwrap()),
            liquidation_penalty_bps: u16::from_le_bytes(data[4..6].try_into().unwrap()),
            liquidation_close_buffer_bps: u16::from_le_bytes(data[6..8].try_into().unwrap()),
        })
    }
}

impl<'a> InstructionData<'a> for ProposeRiskUpdateData {
    const LEN: usize = 8;
}
