use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::EnvFilter;

use crate::error::CommonError;

/// Install a JSON `tracing` subscriber honouring `RUST_LOG` (default `info`).
/// Idempotent — safe to call once per process; a second call is a no-op.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .try_init();
}

/// Install the Prometheus recorder and return a handle a service can render at
/// its own `/metrics` endpoint (no socket is bound here).
pub fn init_metrics() -> Result<PrometheusHandle, CommonError> {
    PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| CommonError::Config(e.to_string()))
}
