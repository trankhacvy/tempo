/// Errors from the pure math layer. Mirrors the subset of the program's
/// `TempoProgramError` that the shared arithmetic can raise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathError {
    Overflow,
    InvalidTick,
    InvalidPrice,
    OracleInvalidAccount,
    OracleFeedMismatch,
    OracleStale,
    OracleFutureTimestamp,
    OracleNegativePrice,
    OracleConfidenceTooWide,
    OracleSoftStale,
}

impl core::fmt::Display for MathError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            MathError::Overflow => "math overflow",
            MathError::InvalidTick => "invalid tick",
            MathError::InvalidPrice => "invalid price",
            MathError::OracleInvalidAccount => "oracle account invalid",
            MathError::OracleFeedMismatch => "oracle feed id mismatch",
            MathError::OracleStale => "oracle price stale",
            MathError::OracleFutureTimestamp => "oracle publish time in the future",
            MathError::OracleNegativePrice => "oracle price not positive",
            MathError::OracleConfidenceTooWide => "oracle confidence interval too wide",
            MathError::OracleSoftStale => "oracle soft-stale (no fresh price)",
        };
        f.write_str(s)
    }
}
