//! A small "cron" task the orchestrator runs: periodically request a devnet
//! airdrop to the master wallet so it stays funded. The master is the source of
//! SOL for the agents and the web faucet's SOL dust, and the keeper drains SOL as
//! it cranks — keeping the master topped up lets it backstop the whole sim.
//!
//! Devnet airdrops are best-effort and heavily rate-limited, so a failed request
//! just logs a warning and the next tick retries.

use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use tokio::sync::watch;

use tempo_common::RpcPool;

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Loop until `shutdown` flips true, requesting `lamports` to `master` every
/// `interval`. The first airdrop fires after one `interval` (the master is funded
/// at provision time, so there is no need to fire immediately).
pub async fn run(
    pool: RpcPool,
    master: Pubkey,
    lamports: u64,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
        match pool
            .call(2, async |rpc| rpc.request_airdrop(&master, lamports).await)
            .await
        {
            Ok(sig) => tracing::info!(
                %master,
                %sig,
                sol = lamports as f64 / LAMPORTS_PER_SOL as f64,
                "master airdrop requested"
            ),
            Err(e) => tracing::warn!(%master, error = %e, "master airdrop failed (devnet rate-limited?)"),
        }
    }
}
