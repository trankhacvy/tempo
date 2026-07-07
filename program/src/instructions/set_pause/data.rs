use pinocchio::error::ProgramError;

use crate::{errors::TempoProgramError, state::Market, traits::InstructionData};

/// Instruction data for SetPause.
///
/// # Layout (little-endian)
/// * `paused` (u8) — the NEW pause bitflag set: bit 0 = `PAUSE_INTAKE`
///   (submit_order + maker-quote writes reject), bit 1 = `PAUSE_ROLL`
///   (start_auction also rejects). `0` fully resumes. Unknown bits are rejected
///   so a future flag can never be set accidentally by an old client.
pub struct SetPauseData {
    pub paused: u8,
}

impl<'a> TryFrom<&'a [u8]> for SetPauseData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        let paused = data[0];
        if paused & !Market::PAUSE_ALL != 0 {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        Ok(Self { paused })
    }
}

impl<'a> InstructionData<'a> for SetPauseData {
    const LEN: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_flags() {
        for flags in [0u8, 1, 2, 3] {
            let d = SetPauseData::try_from(&[flags][..]).unwrap();
            assert_eq!(d.paused, flags);
        }
    }

    #[test]
    fn test_unknown_bits_rejected() {
        assert_eq!(
            SetPauseData::try_from(&[0b100u8][..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_wrong_len_rejected() {
        assert!(SetPauseData::try_from(&[][..]).is_err());
        assert!(SetPauseData::try_from(&[0u8, 0][..]).is_err());
    }
}
