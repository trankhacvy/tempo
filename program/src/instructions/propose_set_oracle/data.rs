use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ProposeSetOracle (plan.md §3.3). Repointing the oracle
/// is the most dangerous admin power in the protocol (whoever controls the
/// oracle controls liquidation prices), so it is staged behind the delay AND
/// only proposable while the market is winding down (`PAUSE_ROLL`).
///
/// # Layout
/// * `new_oracle` ([u8;32]) · `new_feed_id` ([u8;32])
pub struct ProposeSetOracleData {
    pub new_oracle: [u8; 32],
    pub new_feed_id: [u8; 32],
}

impl<'a> TryFrom<&'a [u8]> for ProposeSetOracleData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            new_oracle: data[0..32].try_into().unwrap(),
            new_feed_id: data[32..64].try_into().unwrap(),
        })
    }
}

impl<'a> InstructionData<'a> for ProposeSetOracleData {
    const LEN: usize = 64;
}
