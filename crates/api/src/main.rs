use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use tempo_sdk::Pubkey;
use tokio::sync::{broadcast, watch};
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;

use tempo_api::config::ApiConfig;
use tempo_api::error::ApiError;
use tempo_api::history::NoHistory;
use tempo_api::state::AppState;
use tempo_api::{metrics_defs, routes, watcher};
use tempo_common::telemetry::{init_metrics, init_tracing};
use tempo_common::RpcPool;
use tempo_sdk::{MarketPdas, TempoClient};

#[tokio::main]
async fn main() -> Result<(), ApiError> {
    let cfg = ApiConfig::load()?;
    init_tracing();
    let metrics_handle = init_metrics().map_err(|e| ApiError::Internal(e.to_string()))?;
    metrics_defs::register();

    let market: Pubkey = cfg
        .common
        .market
        .clone()
        .ok_or_else(|| ApiError::Internal("TEMPO_MARKET is required".into()))?
        .parse()
        .map_err(|_| ApiError::Internal("TEMPO_MARKET is not a valid pubkey".into()))?;
    let pdas = MarketPdas::derive(market);

    let pool = RpcPool::from_urls(&cfg.common.rpc_url, cfg.common.commitment_config())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let client = Arc::new(TempoClient::new(
        pool,
        cfg.common.priority_fee_micro_lamports,
    ));

    let (updates, _) = broadcast::channel(cfg.ws_buffer);
    let state = AppState {
        market,
        pdas,
        client,
        live: Arc::new(ArcSwapOption::empty()),
        positions: Arc::new(ArcSwapOption::empty()),
        updates,
        history: Arc::new(NoHistory),
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tracing::info!(market = %market, bind = %cfg.bind_addr, "tempo-api starting");

    let watcher = watcher::run(
        state.clone(),
        Duration::from_millis(cfg.poll_ms),
        shutdown_rx.clone(),
    );
    let positions_watcher = watcher::run_positions(
        state.clone(),
        Duration::from_millis(cfg.position_poll_ms),
        shutdown_rx.clone(),
    );
    let server = serve(&cfg, state, metrics_handle, shutdown_rx.clone());
    let signal = async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl-c handler failed");
        }
        tracing::info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    };

    let (_, _, server_res, _) = tokio::join!(watcher, positions_watcher, server, signal);
    if let Err(e) = server_res {
        tracing::warn!(error = %e, "http server exited with error");
    }
    Ok(())
}

async fn serve(
    cfg: &ApiConfig,
    state: AppState,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let mut app = routes::router(state, metrics_handle, &cfg.cors_origins);

    // Per-IP token-bucket rate limit (needs peer-IP connect info, added below).
    match GovernorConfigBuilder::default()
        .per_second(cfg.rate_limit_rps.max(1) as u64)
        .burst_size(cfg.rate_limit_burst.max(1))
        .finish()
    {
        Some(conf) => {
            app = app.layer(GovernorLayer {
                config: Arc::new(conf),
            });
        }
        None => tracing::warn!("rate-limit config invalid; serving without a limiter"),
    }

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    })
    .await
}
