use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use tokio::sync::watch;

use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::{load_keypair_file, RpcPool};
use tempo_keeper::health::{self, Health};
use tempo_keeper::{funding, metrics_defs, KeeperConfig, KeeperCtx, KeeperError};
use tempo_sdk::{pda, MarketPdas, TempoClient};

#[tokio::main]
async fn main() -> Result<(), KeeperError> {
    let cfg = KeeperConfig::load()?;
    init_tracing();
    let metrics_handle = init_metrics().map_err(KeeperError::Common)?;
    metrics_defs::register();

    let keypair_path = cfg
        .common
        .keypair
        .clone()
        .ok_or_else(|| KeeperError::Config("TEMPO_KEYPAIR is required".into()))?;
    let cranker = Arc::new(load_keypair_file(&keypair_path).map_err(KeeperError::Common)?);

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(KeeperError::Common)?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    let market: Pubkey = cfg
        .common
        .market
        .clone()
        .ok_or_else(|| KeeperError::Config("TEMPO_MARKET is required".into()))?
        .parse()
        .map_err(|_| KeeperError::Config("TEMPO_MARKET is not a valid pubkey".into()))?;
    let pdas = MarketPdas::derive(market);

    let collateral_mint = match cfg.common.collateral_mint.as_deref() {
        Some(s) => Some(s.parse::<Pubkey>().map_err(|_| {
            KeeperError::Config("TEMPO_COLLATERAL_MINT is not a valid pubkey".into())
        })?),
        None => None,
    };
    let vault = collateral_mint.map(|m| pda::vault(&m).0);

    let ctx = KeeperCtx {
        client,
        cranker,
        pdas,
        collateral_mint,
        vault,
        chunk_size: cfg.chunk_size,
        settle_concurrency: cfg.settle_concurrency,
    };

    let health = Health::new(cfg.no_progress_slots);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(market = %market, "tempo-keeper starting");

    // The RPC pool's async-closure call sites trip a higher-ranked-lifetime `Send`
    // limitation under `tokio::spawn`, so the three loops are driven concurrently on
    // the main task via `join!` (they are IO-bound — cooperative scheduling suffices).
    let poll = Duration::from_millis(cfg.poll_interval_ms);
    let keeper = tempo_keeper::run(ctx.clone(), health.clone(), poll, shutdown_rx.clone());
    let funding = funding::run(ctx.clone(), cfg.funding_interval_secs, shutdown_rx.clone());
    let server = health::serve(
        cfg.health_addr.clone(),
        health.clone(),
        metrics_handle,
        shutdown_rx.clone(),
    );
    let signal = async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl-c handler failed");
        }
        tracing::info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    };

    let (_, _, server_res, _) = tokio::join!(keeper, funding, server, signal);
    if let Err(e) = server_res {
        tracing::warn!(error = %e, "health server exited with error");
    }
    Ok(())
}
