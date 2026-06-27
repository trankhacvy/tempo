//! The Tempo reference liquidator: a stateless, replica-safe risk backstop. Each
//! scan reconstructs the underwater set from chain and fires `liquidate` /
//! `liquidate_cross`; it holds no must-persist state, so a crash or a second
//! replica is always safe (the program is the final gate — a `NotLiquidatable`
//! race is benign, not an error). A permissionless *reference* implementation: the
//! exchange's safety never assumes this instance is the one running.

pub mod actions;
pub mod config;
pub mod engine;
pub mod error;
pub mod health;
pub mod metrics_defs;
pub mod snapshot;
pub mod source;

pub use config::LiquidatorConfig;
pub use error::LiquidatorError;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::{self, StreamExt as _};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use tokio::sync::watch;

use tempo_common::Backoff;
use tempo_sdk::{benign, ix, TempoClient};

use crate::engine::LiqAction;
use crate::health::Health;
use crate::snapshot::Scan;
use crate::source::PositionSource;

/// Shared context for one liquidator instance. Clone is cheap (all `Arc`/`Copy`).
#[derive(Clone)]
pub struct LiqCtx {
    pub client: Arc<TempoClient>,
    pub liquidator: Arc<Keypair>,
    /// The liquidator's own collateral ledger (`pda::user_collateral(self)`) — it
    /// receives the liquidation penalty.
    pub liquidator_collateral: Pubkey,
    pub source: Arc<dyn PositionSource>,
    pub markets: Vec<Pubkey>,
    /// The vault PDA for the configured collateral mint, when a money path exists.
    pub vault: Option<Pubkey>,
    pub scan_concurrency: usize,
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Idempotent bootstrap: ensure the liquidator's own collateral ledger exists (it
/// receives penalties). Checks on-chain first to avoid a 120s ConfirmTimeout when
/// the account already exists and simulation rejects with "uninitialized account".
pub async fn ensure_accounts(ctx: &LiqCtx) {
    let uc_pda = tempo_sdk::pda::user_collateral(&ctx.liquidator.pubkey()).0;
    let exists = ctx.client.fetch_account_data_opt(&uc_pda).await
        .map(|opt| opt.is_some())
        .unwrap_or(false);
    if exists {
        tracing::info!("ensure_accounts: liquidator collateral ledger already exists");
        return;
    }
    let pubkey = ctx.liquidator.pubkey();
    let ix = ix::init_collateral(pubkey, pubkey);
    match ctx.client.send(&ctx.liquidator, &[ix]).await {
        Ok(sig) => tracing::info!(%sig, "ensure_accounts: init_collateral ok"),
        Err(e) if benign(&e) => tracing::info!("ensure_accounts: init_collateral benign"),
        Err(e) => tracing::warn!(error = %e, "init_collateral failed"),
    }
}

/// Run the scan loop until `shutdown` flips true.
pub async fn run(ctx: LiqCtx, health: Health, poll: Duration, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = Backoff::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("liquidator shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll) => {
                match scan_once(&ctx).await {
                    Ok(_) => {
                        health.observe();
                        backoff.reset();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "liquidator scan failed");
                        metrics::counter!("liquidator_scan_errors_total").increment(1);
                        health.rpc_down();
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}

/// One scan: price the book, fire isolated liquidations (bounded concurrency), then
/// resolve + fire cross liquidations (serial per owner). Returns the insurance
/// balance for the caller's gauge.
pub async fn scan_once(ctx: &LiqCtx) -> Result<Option<u64>, LiquidatorError> {
    let started = Instant::now();
    let now_ts = unix_now_secs();
    tracing::info!(markets = ctx.markets.len(), "liquidator scan starting");
    // One market/price cache per scan, shared with the cross resolutions below so a
    // market or oracle is fetched at most once per scan (not once per cross owner).
    let mut market_cache = HashMap::new();
    let mut price_cache = HashMap::new();
    let scan = Scan::load(ctx, now_ts, &mut market_cache, &mut price_cache).await?;
    metrics::gauge!("liquidator_positions_scanned").set(scan.isolated.len() as f64);
    if let Some(b) = scan.insurance {
        metrics::gauge!("liquidator_insurance_balance").set(b as f64);
    }

    let actions: Vec<LiqAction> = scan
        .isolated
        .iter()
        .filter(|c| engine::isolated_liquidatable(c))
        .map(|c| LiqAction::Isolated {
            position: c.key,
            owner: c.view.owner,
            market: c.market,
            oracle: c.oracle,
        })
        .collect();
    let underwater_count = actions.len();
    metrics::gauge!("liquidator_underwater_count").set(underwater_count as f64);

    stream::iter(actions)
        .for_each_concurrent(ctx.scan_concurrency, |action| async move {
            actions::liquidate_isolated(ctx, &action).await;
        })
        .await;

    let cross_results: Vec<_> = stream::iter(&scan.cross_owners)
        .map(|owner| {
            let mc = market_cache.clone();
            let pc = price_cache.clone();
            async move {
                (
                    owner,
                    snapshot::resolve_cross(ctx, owner, now_ts, mc, pc).await,
                )
            }
        })
        .buffer_unordered(ctx.scan_concurrency)
        .collect()
        .await;

    for (owner, result) in cross_results {
        match result {
            Ok(Some(account)) => {
                if let Some(legs) = engine::cross_liquidatable(account.balance, &account.members) {
                    actions::liquidate_cross(ctx, owner, &legs).await;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(owner = %owner, error = %e, "cross resolve failed");
                metrics::counter!("liquidator_cross_errors_total").increment(1);
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    metrics::histogram!("liquidator_scan_latency_seconds").record(elapsed);
    tracing::info!(
        isolated = scan.isolated.len(),
        cross_owners = scan.cross_owners.len(),
        underwater = underwater_count,
        elapsed_secs = elapsed,
        "liquidator scan complete"
    );
    Ok(scan.insurance)
}
