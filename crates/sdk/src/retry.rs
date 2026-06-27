//! Shared crank-race classifier. A permissionless crank or quote write that
//! loses a race (wrong phase, already consumed, completeness not yet met,
//! another replica won) is benign and must not back off; an RPC/connectivity
//! error is not benign — the caller should retry/back off. Both the keeper and
//! the reference market maker classify through this one tested function so the
//! D3 (replica-safe) behaviour cannot drift between services.
//!
//! String matching (`is_race_error`) maps to `TempoProgramError` variants:
//!   "custom program error" / "custom error" → any on-chain custom error code
//!   "already"             → AlreadyConsumed, AlreadyProcessed
//!   "wrong phase"         → AuctionWrongPhase
//!   "not found"           → OrderNotFound, AccountNotFound
//!
//! Monitor `keeper_tx_total{result="error"}` after deploy: an unexpected spike
//! means a benign race string was missed and should be added to `is_race_error`.

use crate::error::SdkError;

// TODO(known-issues §4.10): replace string matching with numeric error-code matching
// once the program stabilises its error table and exposes structured codes that the
// SDK can match without parsing the error message string.
fn is_race_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("custom program error")
        || m.contains("custom error")
        || m.contains("already")
        || m.contains("wrong phase")
        || m.contains("not found")
}

/// `true` when `e` is a benign program/transaction race (ignore it), `false`
/// when it is a connectivity or instruction-build error the caller should back off on.
pub fn benign(e: &SdkError) -> bool {
    match e {
        SdkError::Common(tempo_common::CommonError::TxFailed { err, .. }) => is_race_error(err),
        SdkError::Common(tempo_common::CommonError::Rpc(m)) => is_race_error(m),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempo_common::CommonError;

    #[test]
    fn benign_classifies_custom_program_error_in_tx_failed() {
        assert!(benign(&SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: "Error processing Instruction 0: custom program error: 0x6".into(),
        })));
        assert!(benign(&SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: "Custom error: 6".into(),
        })));
    }

    #[test]
    fn benign_classifies_program_error_string() {
        assert!(benign(&SdkError::Common(CommonError::Rpc(
            "Transaction simulation failed: custom program error: 0x1".into()
        ))));
        assert!(benign(&SdkError::Common(CommonError::Rpc(
            "AlreadyProcessed".into()
        ))));
    }

    #[test]
    fn benign_rejects_instruction_build_errors() {
        assert!(!benign(&SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: "Error processing Instruction 0: incorrect program id for instruction".into(),
        })));
        assert!(!benign(&SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: "Error processing Instruction 0: invalid account data".into(),
        })));
    }

    #[test]
    fn benign_unknown_string_is_not_benign() {
        assert!(!benign(&SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: "some completely unexpected error".into(),
        })));
    }

    #[test]
    fn benign_rejects_connectivity_errors() {
        assert!(!benign(&SdkError::Common(CommonError::Rpc(
            "connection timed out".into()
        ))));
        assert!(!benign(&SdkError::Common(CommonError::ConfirmTimeout(
            "sig".into()
        ))));
        assert!(!benign(&SdkError::Decode("bad".into())));
    }
}
