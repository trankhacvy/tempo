//! A small "cron" task the orchestrator runs: periodically request a devnet
//! airdrop to the master wallet so it stays funded. The master is the source of
//! SOL for the agents and the web faucet's SOL dust, and the keeper drains SOL as
//! it cranks — keeping the master topped up lets it backstop the whole sim.
//!
//! Devnet airdrops are best-effort and heavily rate-limited, so a failed request
//! just logs a warning and the next tick retries.

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_system_interface::instruction as system_instruction;
use tokio::sync::watch;

use tempo_common::RpcPool;
use tempo_sdk::TempoClient;

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

/// Refill agent wallets (keeper, market makers, traders, liquidator) from the
/// master whenever their SOL drops below `threshold`, topping each back up to
/// `target`. This is what keeps the keeper — which signs the most transactions and
/// so drains SOL fastest — from running dry and wedging the round in `Settling`.
///
/// Best-effort and idempotent: a failed balance read or transfer just logs and the
/// next tick retries. Requires the master to hold SOL (kept up by [`run`]).
#[allow(clippy::too_many_arguments)]
pub async fn topup_run(
    client: Arc<TempoClient>,
    master: Arc<Keypair>,
    agents: Vec<Pubkey>,
    threshold: u64,
    target: u64,
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
        for agent in &agents {
            let bal = match client.pool().call(2, async |rpc| rpc.get_balance(agent).await).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(%agent, error = %e, "topup: balance check failed");
                    continue;
                }
            };
            if bal >= threshold {
                continue;
            }
            let amount = target.saturating_sub(bal);
            if amount == 0 {
                continue;
            }
            let ix = system_instruction::transfer(&master.pubkey(), agent, amount);
            match client.send(&master, &[ix]).await {
                Ok(sig) => tracing::info!(
                    %agent,
                    %sig,
                    sol = amount as f64 / LAMPORTS_PER_SOL as f64,
                    "topup: refilled agent"
                ),
                Err(e) => tracing::warn!(%agent, error = %e, "topup: transfer failed (master out of SOL?)"),
            }
        }
    }
}
