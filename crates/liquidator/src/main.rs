use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use tokio::sync::watch;

use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::{load_keypair_file, RpcPool};
use tempo_liquidator::health::{self, Health};
use tempo_liquidator::source::ChainScan;
use tempo_liquidator::{metrics_defs, LiqCtx, LiquidatorConfig, LiquidatorError};
use tempo_sdk::{pda, TempoClient};

#[tokio::main]
async fn main() -> Result<(), LiquidatorError> {
    let cfg = LiquidatorConfig::load()?;
    init_tracing();
    let metrics_handle = init_metrics().map_err(LiquidatorError::Common)?;
    metrics_defs::register();

    let keypair_path = cfg
        .common
        .keypair
        .clone()
        .ok_or_else(|| LiquidatorError::Config("TEMPO_KEYPAIR is required".into()))?;
    let liquidator = Arc::new(load_keypair_file(&keypair_path).map_err(LiquidatorError::Common)?);

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(LiquidatorError::Common)?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    let collateral_mint = match cfg.common.collateral_mint.as_deref() {
        Some(s) => Some(s.parse::<Pubkey>().map_err(|_| {
            LiquidatorError::Config("TEMPO_COLLATERAL_MINT is not a valid pubkey".into())
        })?),
        None => None,
    };
    let vault = collateral_mint.map(|m| pda::vault(&m).0);

    let ctx = LiqCtx {
        client: client.clone(),
        liquidator: liquidator.clone(),
        liquidator_collateral: collateral_mint
            .map(|m| pda::user_collateral(&liquidator.pubkey(), &m).0)
            .unwrap_or_default(),
        source: Arc::new(ChainScan::new(client)),
        markets: cfg.markets.clone(),
        vault,
        collateral_mint,
        scan_concurrency: cfg.scan_concurrency,
    };

    tempo_liquidator::ensure_accounts(&ctx).await;

    // `--once`: a single scan for cron/CI smoke, then exit.
    if std::env::args().any(|a| a == "--once") {
        tempo_liquidator::scan_once(&ctx).await?;
        return Ok(());
    }

    let health = Health::new(cfg.stale_scan_secs);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tracing::info!(
        markets = cfg.markets.len(),
        liquidator = %liquidator.pubkey(),
        "tempo-liquidator starting"
    );

    let poll = Duration::from_millis(cfg.poll_interval_ms);
    let scanner = tempo_liquidator::run(ctx, health.clone(), poll, shutdown_rx.clone());
    let server = health::serve(
        cfg.health_addr.clone(),
        health,
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

    let (_, server_res, _) = tokio::join!(scanner, server, signal);
    if let Err(e) = server_res {
        tracing::warn!(error = %e, "health server exited with error");
    }
    Ok(())
}
