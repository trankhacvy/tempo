use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::sync::watch;

/// Liveness/readiness for a trader. Readiness = RPC reachable; a trader that simply
/// has nothing to submit (off-Collect, or skipped a round) is still healthy, so the
/// gate only fails on RPC loss. Mirrors the keeper/mm health server shape.
#[derive(Clone)]
pub struct Health(Arc<Inner>);

struct Inner {
    rpc_ok: AtomicBool,
    current_auction: AtomicU64,
}

impl Health {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            rpc_ok: AtomicBool::new(true),
            current_auction: AtomicU64::new(0),
        }))
    }

    pub fn observe(&self, current_auction: u64) {
        self.0.rpc_ok.store(true, Ordering::Relaxed);
        self.0
            .current_auction
            .store(current_auction, Ordering::Relaxed);
    }

    pub fn rpc_down(&self) {
        self.0.rpc_ok.store(false, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.0.rpc_ok.load(Ordering::Relaxed)
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

/// Serve `/healthz`, `/readyz`, and `/metrics` until shutdown.
pub async fn serve(
    addr: String,
    health: Health,
    metrics_handle: PrometheusHandle,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let readyz_health = health.clone();
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let h = readyz_health.clone();
                async move {
                    if h.is_ready() {
                        (StatusCode::OK, "ready")
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let m = metrics_handle.clone();
                async move { m.render() }
            }),
        );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_until_rpc_drops() {
        let h = Health::new();
        h.observe(5);
        assert!(h.is_ready());
        h.rpc_down();
        assert!(!h.is_ready());
    }
}
