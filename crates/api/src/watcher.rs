use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use tempo_common::Backoff;

use crate::state::{AppState, LiveState};

/// The single per-market poller: every `poll` it loads a fresh [`LiveState`],
/// stores it in the shared `ArcSwap`, and broadcasts it to WS subscribers when
/// the content fingerprint changes. REST handlers never touch RPC — they read
/// the `ArcSwap`, so RPC load is bounded by this one task, not by client count.
pub async fn run(state: AppState, poll: Duration, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = Backoff::new();
    let mut last_fingerprint: Option<u64> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("api watcher shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll) => {
                match tick(&state).await {
                    Ok(live) => {
                        backoff.reset();
                        let fp = live.fingerprint();
                        let changed = last_fingerprint != Some(fp);
                        last_fingerprint = Some(fp);
                        let arc = Arc::new(live);
                        state.live.store(Some(arc.clone()));
                        if changed {
                            let _ = state.updates.send(arc);
                        }
                        metrics::counter!("api_watcher_poll_total", "result" => "ok").increment(1);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "api watcher poll failed");
                        metrics::counter!("api_watcher_poll_total", "result" => "error").increment(1);
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}

async fn tick(state: &AppState) -> Result<LiveState, tempo_sdk::SdkError> {
    let slot = state.client.current_slot().await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let live = LiveState::load(&state.client, &state.pdas, slot, now).await?;
    metrics::gauge!("api_live_phase").set(live.market.phase as f64);
    Ok(live)
}

/// Slow-cadence position GPA scan (default every 5s). Runs independently from
/// the fast auction-state watcher so a getProgramAccounts call does not block
/// the histogram/phase poll.
pub async fn run_positions(state: AppState, poll: Duration, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = Backoff::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("api position watcher shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(poll) => {
                match state.client.fetch_positions(&state.pdas.market).await {
                    Ok(positions) => {
                        state.positions.store(Some(Arc::new(positions)));
                        backoff.reset();
                        metrics::counter!("api_position_poll_total", "result" => "ok")
                            .increment(1);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "position scan failed");
                        metrics::counter!("api_position_poll_total", "result" => "error")
                            .increment(1);
                        backoff.sleep().await;
                    }
                }
            }
        }
    }
}
