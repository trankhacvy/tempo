use std::time::{Duration, Instant};

use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_commitment_config::CommitmentLevel;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::{Keypair, Signature, Signer};
use solana_sdk::transaction::Transaction;

use crate::error::CommonError;
use crate::rpc::{classify_error, RpcPool};

/// Compute budget per tx. Must cover the heaviest single instruction the keeper
/// sends: a money-path `settle_maker_quote` / `finalize_clear` (256 ticks) folds in
/// the position update, funding, socialized-loss, insurance, and fee math, which
/// can run well past 100k CU. Under-sizing here makes the tx FAIL PREFLIGHT (the
/// pool sends with `skip_preflight: false`), so it never lands and the round wedges
/// in `Settling`. 400k stays far under the 1.4M/tx ceiling; you are billed for CU
/// consumed, not requested, so over-provisioning the limit is free.
pub const DEFAULT_CU_LIMIT: u32 = 400_000;

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
const CONFIRM_POLL: Duration = Duration::from_millis(400);

/// Builds, signs, sends, and confirms transactions over an [`RpcPool`], with
/// compute-budget sizing, optional priority fee, and pool failover.
pub struct TxSender<'a> {
    pool: &'a RpcPool,
    priority_fee_micro_lamports: u64,
}

impl<'a> TxSender<'a> {
    pub fn new(pool: &'a RpcPool, priority_fee_micro_lamports: u64) -> Self {
        Self {
            pool,
            priority_fee_micro_lamports,
        }
    }

    /// Prepend a right-sized compute-budget limit (and a priority fee when
    /// configured), sign with a fresh blockhash, broadcast across the pool,
    /// and confirm by polling the signature status.
    pub async fn send(
        &self,
        payer: &Keypair,
        ixs: &[Instruction],
        cu_limit: u32,
    ) -> Result<Signature, CommonError> {
        let mut all = Vec::with_capacity(ixs.len() + 2);
        all.push(ComputeBudgetInstruction::set_compute_unit_limit(cu_limit));
        if self.priority_fee_micro_lamports > 0 {
            all.push(ComputeBudgetInstruction::set_compute_unit_price(
                self.priority_fee_micro_lamports,
            ));
        }
        all.extend_from_slice(ixs);

        let start_idx = self.pool.cursor();
        let blockhash = self
            .pool
            .call_on(start_idx, 6, true, async |rpc| {
                rpc.get_latest_blockhash().await
            })
            .await?;

        let tx =
            Transaction::new_signed_with_payer(&all, Some(&payer.pubkey()), &[payer], blockhash);
        let sig = tx.signatures[0];

        self.broadcast(start_idx, &tx).await;

        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            let status = self
                .pool
                .call(8, async |rpc| rpc.get_signature_status(&sig).await)
                .await?;
            match status {
                Some(Ok(())) => return Ok(sig),
                Some(Err(e)) => {
                    return Err(CommonError::TxFailed {
                        sig: sig.to_string(),
                        err: e.to_string(),
                    })
                }
                None => {
                    if Instant::now() >= deadline {
                        return Err(CommonError::ConfirmTimeout(sig.to_string()));
                    }
                    tokio::time::sleep(CONFIRM_POLL).await;
                }
            }
        }
    }

    /// Like [`send`](Self::send) but rebuilds the transaction (fresh blockhash)
    /// on a blockhash/expiry/already-processed error — the contended hot path
    /// (port of `tx.ts::sendWithConflictRetry`).
    pub async fn send_with_conflict_retry(
        &self,
        payer: &Keypair,
        ixs: &[Instruction],
        cu_limit: u32,
        attempts: usize,
    ) -> Result<Signature, CommonError> {
        for i in 0..attempts.max(1) {
            match self.send(payer, ixs, cu_limit).await {
                Ok(sig) => return Ok(sig),
                Err(e) => {
                    if i + 1 >= attempts || !is_conflict(&e) {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(300 * (i as u64 + 1))).await;
                }
            }
        }
        unreachable!("loop returns on the final attempt")
    }

    /// Broadcast a signed tx across the pool: rotate off a throttled key (a
    /// 429-rejected send never landed), but return on a transient timeout (it
    /// may have landed — let the confirm poll decide). Port of `sendOnPool`.
    async fn broadcast(&self, start_idx: usize, tx: &Transaction) {
        let cfg = RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            ..Default::default()
        };
        let tries = self.pool.len() + 2;
        for i in 0..tries {
            match self
                .pool
                .client(start_idx + i)
                .send_transaction_with_config(tx, cfg)
                .await
            {
                Ok(_) => return,
                Err(e) => {
                    let msg = e.to_string();
                    let (rate, transient) = classify_error(&msg);
                    if !rate && !transient {
                        tracing::debug!(error = %msg, "broadcast: non-retryable send error (simulation failure or invalid tx)");
                    }
                    if rate {
                        tokio::time::sleep(Duration::from_millis(80)).await;
                        continue;
                    }
                    if transient {
                        return;
                    }
                    return;
                }
            }
        }
    }
}

fn is_conflict(e: &CommonError) -> bool {
    let m = match e {
        CommonError::TxFailed { err, .. } => err.to_ascii_lowercase(),
        CommonError::ConfirmTimeout(_) => return true,
        CommonError::Rpc(s) => s.to_ascii_lowercase(),
        _ => return false,
    };
    m.contains("blockhash")
        || m.contains("block height exceeded")
        || m.contains("expired")
        || m.contains("already processed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_conflict() {
        assert!(is_conflict(&CommonError::Rpc("Blockhash not found".into())));
        assert!(is_conflict(&CommonError::ConfirmTimeout("sig".into())));
        assert!(is_conflict(&CommonError::TxFailed {
            sig: "s".into(),
            err: "block height exceeded".into(),
        }));
        assert!(!is_conflict(&CommonError::Rpc("insufficient funds".into())));
    }

    #[test]
    fn test_priority_fee_stored_in_sender() {
        let pool = crate::rpc::RpcPool::from_urls(
            "https://api.devnet.solana.com",
            solana_commitment_config::CommitmentConfig::confirmed(),
        )
        .unwrap();
        assert_eq!(TxSender::new(&pool, 1000).priority_fee_micro_lamports, 1000);
        assert_eq!(TxSender::new(&pool, 0).priority_fee_micro_lamports, 0);
    }
}
