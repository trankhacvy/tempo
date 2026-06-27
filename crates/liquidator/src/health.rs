use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::sync::watch;

/// Liveness/readiness for the liquidator. Readiness = the RPC is reachable AND a
/// full scan completed within `stale_scan_secs` (a liquidator that has not scanned
/// recently is not safely backstopping risk, so it reports not-ready).
#[derive(Clone)]
pub struct Health(Arc<Inner>);

struct Inner {
    rpc_ok: AtomicBool,
    last_scan_unix: AtomicU64,
    scanned: AtomicBool,
    stale_scan_secs: u64,
}

impl Health {
    pub fn new(stale_scan_secs: u64) -> Self {
        Self(Arc::new(Inner {
            rpc_ok: AtomicBool::new(true),
            last_scan_unix: AtomicU64::new(0),
            scanned: AtomicBool::new(false),
            stale_scan_secs,
        }))
    }

    /// Record a completed scan at `now_unix` (also clears any RPC-down flag).
    pub fn observe_at(&self, now_unix: u64) {
        self.0.rpc_ok.store(true, Ordering::Relaxed);
        self.0.last_scan_unix.store(now_unix, Ordering::Relaxed);
        self.0.scanned.store(true, Ordering::Relaxed);
    }

    pub fn observe(&self) {
        self.observe_at(unix_now());
    }

    pub fn rpc_down(&self) {
        self.0.rpc_ok.store(false, Ordering::Relaxed);
    }

    pub fn is_ready_at(&self, now_unix: u64) -> bool {
        let i = &self.0;
        if !i.rpc_ok.load(Ordering::Relaxed) {
            return false;
        }
        if !i.scanned.load(Ordering::Relaxed) {
            return false;
        }
        let last = i.last_scan_unix.load(Ordering::Relaxed);
        now_unix.saturating_sub(last) <= i.stale_scan_secs
    }

    pub fn is_ready(&self) -> bool {
        self.is_ready_at(unix_now())
    }
}

/// Seconds since the Unix epoch (saturating to 0 on a pre-epoch clock).
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Serve `/healthz` (process up), `/readyz` (RPC reachable + recent scan), and
/// `/metrics` (Prometheus render) until shutdown.
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
    fn not_ready_until_first_scan() {
        let h = Health::new(30);
        assert!(!h.is_ready_at(100), "no scan yet → not ready");
        h.observe_at(100);
        assert!(h.is_ready_at(110));
    }

    #[test]
    fn stale_scan_trips_not_ready() {
        let h = Health::new(30);
        h.observe_at(100);
        assert!(h.is_ready_at(125), "within window");
        assert!(!h.is_ready_at(140), "31s later → stale");
        h.observe_at(140);
        assert!(h.is_ready_at(150), "fresh scan resets the clock");
    }

    #[test]
    fn rpc_down_trips_not_ready() {
        let h = Health::new(30);
        h.observe_at(100);
        assert!(h.is_ready_at(105));
        h.rpc_down();
        assert!(!h.is_ready_at(105));
    }
}
