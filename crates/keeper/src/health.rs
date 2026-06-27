use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::sync::watch;

/// Liveness/readiness state shared between the keeper loop and the HTTP server. The
/// freeze watchdog: readiness flips false if work is pending but the on-chain state
/// fingerprint has not changed for more than `no_progress_slots`.
#[derive(Clone)]
pub struct Health(Arc<Inner>);

struct Inner {
    last_fingerprint: AtomicU64,
    last_progress_slot: AtomicU64,
    now_slot: AtomicU64,
    pending: AtomicBool,
    rpc_ok: AtomicBool,
    initialized: AtomicBool,
    no_progress_slots: u64,
}

impl Health {
    pub fn new(no_progress_slots: u64) -> Self {
        Self(Arc::new(Inner {
            last_fingerprint: AtomicU64::new(0),
            last_progress_slot: AtomicU64::new(0),
            now_slot: AtomicU64::new(0),
            pending: AtomicBool::new(false),
            rpc_ok: AtomicBool::new(true),
            initialized: AtomicBool::new(false),
            no_progress_slots,
        }))
    }

    /// Record a successful tick. Progress = the fingerprint changed (or the first
    /// observation); that resets the freeze clock.
    pub fn observe(&self, fingerprint: u64, pending: bool, now_slot: u64) {
        let i = &self.0;
        i.rpc_ok.store(true, Ordering::Relaxed);
        i.now_slot.store(now_slot, Ordering::Relaxed);
        i.pending.store(pending, Ordering::Relaxed);
        // Compute both unconditionally — a short-circuited `||` would skip storing
        // the fingerprint on the first observe and falsely report progress next tick.
        let prev = i.last_fingerprint.swap(fingerprint, Ordering::Relaxed);
        let first = !i.initialized.swap(true, Ordering::Relaxed);
        if first || prev != fingerprint {
            i.last_progress_slot.store(now_slot, Ordering::Relaxed);
        }
        metrics::gauge!("keeper_slots_since_progress").set(self.slots_since_progress() as f64);
    }

    /// Mark the RPC unreachable (a failed tick) — readiness goes false until the
    /// next successful observe.
    pub fn rpc_down(&self) {
        self.0.rpc_ok.store(false, Ordering::Relaxed);
    }

    pub fn slots_since_progress(&self) -> u64 {
        self.0
            .now_slot
            .load(Ordering::Relaxed)
            .saturating_sub(self.0.last_progress_slot.load(Ordering::Relaxed))
    }

    pub fn is_ready(&self) -> bool {
        let i = &self.0;
        if !i.rpc_ok.load(Ordering::Relaxed) {
            return false;
        }
        if i.pending.load(Ordering::Relaxed) && self.slots_since_progress() > i.no_progress_slots {
            return false;
        }
        true
    }
}

/// Serve `/healthz` (process up), `/readyz` (RPC reachable + not frozen), and
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
    fn freeze_watchdog_trips_when_pending_and_stalled() {
        let h = Health::new(10);
        // First observe with work pending — fresh, ready.
        h.observe(1, true, 100);
        assert!(h.is_ready());
        // Same fingerprint, work still pending, 5 slots later — within window.
        h.observe(1, true, 105);
        assert!(h.is_ready());
        // 11 slots with no fingerprint change while pending → frozen.
        h.observe(1, true, 111);
        assert!(!h.is_ready());
        // A fingerprint change resets the clock.
        h.observe(2, true, 112);
        assert!(h.is_ready());
    }

    #[test]
    fn idle_is_ready_even_when_stalled() {
        let h = Health::new(10);
        h.observe(1, false, 100);
        h.observe(1, false, 999); // no work pending → never "frozen"
        assert!(h.is_ready());
    }

    #[test]
    fn rpc_down_is_not_ready() {
        let h = Health::new(10);
        h.observe(1, false, 100);
        assert!(h.is_ready());
        h.rpc_down();
        assert!(!h.is_ready());
    }
}
