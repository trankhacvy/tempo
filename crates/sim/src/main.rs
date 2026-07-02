use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use tokio::sync::watch;

use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::{load_keypair_file, RpcPool};
use tempo_sdk::{MarketPdas, TempoClient};

use tempo_sim::config::SimConfig;
use tempo_sim::error::SimError;
use tempo_sim::health::{self, Health};
use tempo_sim::{metrics_defs, TraderCtx};

#[tokio::main]
async fn main() -> Result<(), SimError> {
    let cfg = SimConfig::load()?;
    init_tracing();
    let metrics_handle = init_metrics().map_err(SimError::Common)?;
    metrics_defs::register();

    let keypair_path = cfg
        .common
        .keypair
        .clone()
        .ok_or_else(|| SimError::Config("TEMPO_KEYPAIR is required".into()))?;
    let trader = Arc::new(load_keypair_file(&keypair_path).map_err(SimError::Common)?);

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(SimError::Common)?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    let market: Pubkey = cfg
        .common
        .market
        .clone()
        .ok_or_else(|| SimError::Config("TEMPO_MARKET is required".into()))?
        .parse()
        .map_err(|_| SimError::Config("TEMPO_MARKET is not a valid pubkey".into()))?;
    let pdas = MarketPdas::derive(market);

    let collateral_mint =
        match cfg.common.collateral_mint.as_deref() {
            Some(s) => Some(s.parse::<Pubkey>().map_err(|_| {
                SimError::Config("TEMPO_COLLATERAL_MINT is not a valid pubkey".into())
            })?),
            None => None,
        };
    let ctx = TraderCtx {
        client,
        trader,
        pdas,
        collateral_mint,
        // Route to this trader's canonical shard (Stage A). This standalone bin has no
        // artifact, so it reads the shard count from the same env the provisioner used;
        // it must match, or the trader submits to the wrong shard.
        num_slab_shards: std::env::var("TEMPO_SIM_NUM_SHARDS")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(1)
            .clamp(1, 16),
        cfg: cfg.trader_config(),
        seed: cfg.seed,
    };

    let health = Health::new();

    // `--once`: a single tick for smoke-testing, then exit.
    if std::env::args().any(|a| a == "--once") {
        tracing::info!(market = %market, "tempo-sim trader: single tick (--once)");
        tempo_sim::run_once(&ctx, &health).await?;
        return Ok(());
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(market = %market, persona = cfg.persona.as_str(), "tempo-sim trader starting");

    let poll = Duration::from_millis(cfg.poll_ms);
    let trader_loop = tempo_sim::run(ctx, health.clone(), poll, shutdown_rx.clone());
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
        let _ = shutdown_tx.send(true);
    };

    let (_, server_res, _) = tokio::join!(trader_loop, server, signal);
    if let Err(e) = server_res {
        tracing::warn!(error = %e, "health server exited with error");
    }
    Ok(())
}
