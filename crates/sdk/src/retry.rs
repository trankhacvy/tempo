//! Shared crank-race classifier. A permissionless crank or quote write that
//! loses a race (wrong phase, already consumed, completeness not yet met,
//! another replica won) is benign and must not back off; an RPC/connectivity
//! error is not benign — the caller should retry/back off. Both the keeper and
//! the reference market maker classify through this one tested function so the
//! D3 (replica-safe) behaviour cannot drift between services.
//!
//! P5.3 (known-issues §4.10): classification is now **structured** — the on-chain
//! custom error CODE is parsed out of the transaction error and matched against
//! the explicit [`BENIGN_CODES`] allowlist. A custom error whose code is NOT on
//! the list is a REAL error (it used to be silently swallowed by the old
//! "any custom program error is benign" substring rule — an `InsuranceInsolvent`
//! or `VaultInvariantViolated` must surface, not be classified as a race). The
//! substring matcher survives only as the fallback for CODE-LESS transport
//! errors (blockhash expiry, `AlreadyProcessed`, node-behind races).
//!
//! Monitor `keeper_tx_total{result="error"}` after deploy: an unexpected spike
//! means a benign race code was missed and should be added to `BENIGN_CODES`.

use crate::error::SdkError;

/// Crank races that mean "someone else did the work first" (or "the state moved
/// on") — skip/retry-next-tick, never back off, never alert. Codes are
/// `TempoProgramError` discriminants; keep in sync with `program/src/errors.rs`.
pub const BENIGN_CODES: &[u32] = &[
    3,  // AuctionWrongPhase      — phase advanced under us
    5,  // OrderNotFound          — settled/cancelled/reaped first
    9,  // AuctionNotComplete     — crank raced the completeness gate
    10, // OrderAlreadyAccumulated — another cranker folded it first
    16, // AuctionIdMismatch      — round rolled under us (incl. a re-emitted reset_shard)
    17, // InvalidOrderStatus     — order raced to a later status
    25, // NotLiquidatable        — liquidation raced (position healed / already closed)
];

/// Extract the on-chain custom error code from a stringified
/// `TransactionError`, across the formats the RPC stack actually produces:
///   "custom program error: 0x2e"   (Display of `InstructionError::Custom`, preflight logs)
///   "Custom(46)"                   (Debug form)
///   {"Custom":46}                  (JSON-RPC error body)
///   "custom error: 46"             (legacy client Display)
/// Returns `None` for a code-less (transport-level) error.
fn custom_code(msg: &str) -> Option<u32> {
    let m = msg.to_ascii_lowercase();
    let parse_at = |rest: &str, radix: u32| -> Option<u32> {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        u32::from_str_radix(&digits, radix).ok()
    };
    if let Some(i) = m.find("custom program error: 0x") {
        return parse_at(&m[i + "custom program error: 0x".len()..], 16);
    }
    if let Some(i) = m.find("\"custom\":") {
        return parse_at(&m[i + "\"custom\":".len()..], 10);
    }
    if let Some(i) = m.find("custom(") {
        return parse_at(&m[i + "custom(".len()..], 10);
    }
    if let Some(i) = m.find("custom error: ") {
        return parse_at(&m[i + "custom error: ".len()..], 10);
    }
    None
}

/// Code-less fallback: transport/runtime races with no custom code. Kept
/// narrow — these strings come from the Solana runtime, not this program.
///   "already"     → AlreadyProcessed (another replica landed the identical tx)
///   "wrong phase" → phase-guard messages surfaced without a code
///   "not found"   → account/order raced away
///   "blockhash"   → blockhash expired mid-confirm (rebuild + resend next tick)
fn is_transport_race(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("already")
        || m.contains("wrong phase")
        || m.contains("not found")
        || m.contains("blockhash not found")
}

fn is_race_error(msg: &str) -> bool {
    match custom_code(msg) {
        // Structured path: a coded program error is benign IFF allowlisted.
        Some(code) => BENIGN_CODES.contains(&code),
        // Code-less path: the narrow transport-race fallback.
        None => is_transport_race(msg),
    }
}

/// `true` when `e` is a benign program/transaction race (ignore it), `false`
/// when it is a real program error, connectivity failure, or instruction-build
/// error the caller should surface/back off on.
pub fn benign(e: &SdkError) -> bool {
    match e {
        SdkError::Common(tempo_common::CommonError::TxFailed { err, .. }) => is_race_error(err),
        SdkError::Common(tempo_common::CommonError::Rpc(m)) => is_race_error(m),
        // A race can be caught at PREFLIGHT too (the simulation runs the program
        // and returns the same coded error a landed tx would) — e.g. the P5.2
        // keeper re-emitting reset_shard for an already-reset shard fails
        // simulation with AuctionIdMismatch(0x10). Same classification rules.
        SdkError::Common(tempo_common::CommonError::SimulationFailed(m)) => is_race_error(m),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempo_common::CommonError;

    fn tx_failed(err: &str) -> SdkError {
        SdkError::Common(CommonError::TxFailed {
            sig: "s".into(),
            err: err.into(),
        })
    }

    #[test]
    fn benign_allowlists_each_race_code() {
        // Every allowlisted code classifies benign in every wire format.
        for &code in BENIGN_CODES {
            assert!(
                benign(&tx_failed(&format!(
                    "Error processing Instruction 0: custom program error: {code:#x}"
                ))),
                "hex form, code {code}"
            );
            assert!(
                benign(&tx_failed(&format!("InstructionError(0, Custom({code}))"))),
                "debug form, code {code}"
            );
            assert!(
                benign(&SdkError::Common(CommonError::Rpc(format!(
                    "{{\"InstructionError\":[0,{{\"Custom\":{code}}}]}}"
                )))),
                "json form, code {code}"
            );
        }
    }

    #[test]
    fn benign_rejects_non_allowlisted_codes_as_real() {
        // The whole point of P5.3: a REAL program error must surface even though
        // it is a "custom program error" string the old matcher swallowed.
        for code in [33u32, 51, 2, 46, 50] {
            // InsuranceInsolvent(33), VaultInvariantViolated(51), MarketPaused(2),
            // OrderAlreadyExpired(46), OpenInterestCapExceeded(50)
            assert!(
                !benign(&tx_failed(&format!(
                    "Error processing Instruction 0: custom program error: {code:#x}"
                ))),
                "code {code} must be a real error"
            );
        }
    }

    #[test]
    fn benign_classifies_custom_program_error_in_tx_failed() {
        // Format-drift regression (kept from the string-matcher era): the exact
        // preflight/Display strings the RPC stack produces for an allowlisted code.
        assert!(benign(&tx_failed(
            "Error processing Instruction 0: custom program error: 0x5"
        )));
        assert!(benign(&tx_failed("Custom error: 25")));
    }

    #[test]
    fn benign_classifies_program_error_string() {
        assert!(benign(&SdkError::Common(CommonError::Rpc(
            "Transaction simulation failed: custom program error: 0x3".into()
        ))));
        // Code-less transport race: another replica landed the identical tx.
        assert!(benign(&SdkError::Common(CommonError::Rpc(
            "AlreadyProcessed".into()
        ))));
    }

    #[test]
    fn benign_transport_fallback_stays_narrow() {
        // Code-less strings the fallback still classifies as races…
        assert!(benign(&tx_failed("Blockhash not found")));
        assert!(benign(&tx_failed("order not found in slab")));
        // …and code-less strings it must NOT swallow.
        assert!(!benign(&tx_failed(
            "Error processing Instruction 0: incorrect program id for instruction"
        )));
        assert!(!benign(&tx_failed(
            "Error processing Instruction 0: invalid account data"
        )));
    }

    #[test]
    fn benign_unknown_string_is_not_benign() {
        assert!(!benign(&tx_failed("some completely unexpected error")));
    }

    #[test]
    fn benign_classifies_preflight_simulation_races() {
        // The exact live shape from the P5.2 devnet run: a re-emitted
        // reset_shard rejected at preflight with AuctionIdMismatch (0x10).
        assert!(benign(&SdkError::Common(CommonError::SimulationFailed(
            "RPC response error -32002: Transaction simulation failed: \
             Error processing Instruction 0: custom program error: 0x10"
                .into()
        ))));
        // A REAL coded error at preflight still surfaces.
        assert!(!benign(&SdkError::Common(CommonError::SimulationFailed(
            "Transaction simulation failed: custom program error: 0x21".into()
        ))));
        // A code-less simulation failure (build error) is not a race.
        assert!(!benign(&SdkError::Common(CommonError::SimulationFailed(
            "invalid account data for instruction".into()
        ))));
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

    #[test]
    fn custom_code_parses_all_wire_formats() {
        assert_eq!(
            custom_code("Error processing Instruction 1: custom program error: 0x2e"),
            Some(46)
        );
        assert_eq!(custom_code("InstructionError(0, Custom(17))"), Some(17));
        assert_eq!(
            custom_code("{\"InstructionError\":[0,{\"Custom\":9}]}"),
            Some(9)
        );
        assert_eq!(custom_code("Custom error: 25"), Some(25));
        assert_eq!(custom_code("Blockhash not found"), None);
    }
}
