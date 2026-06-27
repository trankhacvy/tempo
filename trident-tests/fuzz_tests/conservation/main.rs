//! Trident transaction-level fuzz target.
//!
//! Drives a random sequence of Tempo instructions through the `trident-svm`
//! backend (which runs the compiled Pinocchio SBF program) and, after each
//! settled round, asserts the protocol's load-bearing invariants. This is the
//! automated, sequence-level complement to the host property fuzzes and the
//! LiteSVM integration tests (which assert the same properties point-wise).
//!
//! Run with the Trident CLI (not `cargo test`):
//!     cd program && cargo-build-sbf
//!     cd ../trident-tests && trident fuzz run conservation
//!
//! Invariants enforced after every transaction / settled round:
//!   1. Solvency:   vault_token_balance >= Σ user_collateral.balance + insurance
//!   2. OI balance: oi_long == oi_short once a round is fully settled
//!   3. Liveness:   no instruction panics (the SVM aborts on a real panic)
//!   4. No-leak:    a `settle bad debt` / `liq bad debt` log only ever appears
//!                  alongside a matching social-loss index increase (ADL), never
//!                  as a silently-dropped shortfall.
//!
//! The instruction set, account derivations, and byte offsets mirror the LiteSVM
//! harness in `tests/integration-tests/src/lib.rs`.

use trident_fuzz::fuzzing::*;

mod invariants;
use invariants::Invariants;

/// One fuzzed Tempo instruction. The fuzzer fills the fields from raw entropy;
/// each variant maps to a real Tempo instruction discriminator.
#[derive(arbitrary::Arbitrary, Debug, Clone)]
enum FuzzInstruction {
    Deposit { trader: u8, amount: u16 },
    SubmitOrder { trader: u8, side: u8, is_maker: u8, price: u16, qty: u16 },
    ProcessChunk { max_count: u8 },
    FinalizeClear,
    SettleFill { order_id: u16 },
    StartAuction,
    UpdateFunding { oracle_price: u16 },
    Liquidate { target: u8 },
}

/// Trident flow harness. `client` is the trident-svm execution backend.
#[derive(FuzzTestMethods, Default)]
struct FuzzTest {
    client: TridentSVM,
    inv: Invariants,
}

#[flow_executor]
impl FuzzTest {
    /// One-time setup: load the program, create the market + vault + a pool of
    /// funded traders.
    #[init]
    fn start(&mut self) {
        self.inv.bootstrap(&mut self.client);
    }

    /// A fuzzed sequence: apply each instruction, then check the invariants. A
    /// transaction that the program rejects (e.g. a stale/late settle) is a clean
    /// no-op (SVM rollback) and is fine; a panic or a broken invariant is a bug.
    #[flow]
    fn flow_round(&mut self, fuzzer: &mut FuzzerData) {
        let ixs: Vec<FuzzInstruction> = fuzzer.arbitrary().unwrap_or_default();
        for ix in ixs {
            // Best-effort apply; rejections roll back and are expected.
            let _ = self.inv.apply(&mut self.client, &ix);
            // Invariant 1 + 4 hold after EVERY transaction.
            self.inv.assert_solvency(&self.client);
            self.inv.assert_no_unbacked_bad_debt(&self.client);
        }
        // Invariant 2 holds once the round is fully settled.
        if self.inv.round_is_settled(&self.client) {
            self.inv.assert_oi_balanced(&self.client);
        }
    }
}

fn main() {
    FuzzTest::fuzz();
}
