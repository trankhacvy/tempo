use tempo_common::env_parse;

use crate::error::KeeperError;

/// Keeper configuration: the shared `tempo_common::Config` (figment / `TEMPO_*`
/// env) plus keeper-specific cadence knobs, also read from `TEMPO_*` env with
/// production-safe defaults.
#[derive(Debug, Clone)]
pub struct KeeperConfig {
    pub common: tempo_common::Config,
    pub poll_interval_ms: u64,
    pub settle_concurrency: usize,
    pub chunk_size: u32,
    pub funding_interval_secs: u64,
    pub no_progress_slots: u64,
    pub health_addr: String,
}

impl KeeperConfig {
    pub fn load() -> Result<Self, KeeperError> {
        let common = tempo_common::Config::load().map_err(KeeperError::Common)?;
        Ok(Self {
            common,
            poll_interval_ms: env_parse("TEMPO_CRANK_POLL_MS", 800),
            settle_concurrency: env_parse("TEMPO_SETTLE_CONCURRENCY", 8),
            chunk_size: env_parse("TEMPO_CHUNK_SIZE", 256),
            funding_interval_secs: env_parse("TEMPO_FUNDING_INTERVAL_SECS", 60),
            no_progress_slots: env_parse("TEMPO_NO_PROGRESS_SLOTS", 300),
            health_addr: std::env::var("TEMPO_HEALTH_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
        })
    }
}
