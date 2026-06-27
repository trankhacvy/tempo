use tempo_common::env_parse;

use crate::error::ApiError;

/// API configuration: the shared `tempo_common::Config` (rpc/market) plus
/// HTTP-specific knobs, read from `TEMPO_API_*` env with production-safe defaults.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub common: tempo_common::Config,
    pub bind_addr: String,
    pub poll_ms: u64,
    pub ws_buffer: usize,
    pub rate_limit_rps: u32,
    pub rate_limit_burst: u32,
    pub cors_origins: Vec<String>,
    /// How often to run the positions GPA scan. Positions change only on fills
    /// and funding, so a slower cadence (default 5s) is fine and avoids hammering
    /// the RPC with expensive getProgramAccounts on every fast-path tick.
    pub position_poll_ms: u64,
}

impl ApiConfig {
    pub fn load() -> Result<Self, ApiError> {
        let common = tempo_common::Config::load().map_err(|e| ApiError::Internal(e.to_string()))?;
        let cors_origins = std::env::var("TEMPO_API_CORS")
            .unwrap_or_else(|_| "*".to_string())
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        Ok(Self {
            common,
            bind_addr: std::env::var("TEMPO_API_BIND")
                .unwrap_or_else(|_| "0.0.0.0:8088".to_string()),
            poll_ms: env_parse("TEMPO_API_POLL_MS", 400),
            ws_buffer: env_parse("TEMPO_API_WS_BUFFER", 1024),
            rate_limit_rps: env_parse("TEMPO_API_RATE_RPS", 50),
            rate_limit_burst: env_parse("TEMPO_API_RATE_BURST", 100),
            cors_origins,
            position_poll_ms: env_parse("TEMPO_API_POSITION_POLL_MS", 5000),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_cors_parsing() {
        let parsed: Vec<String> = "https://a.app, https://b.app"
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        assert_eq!(parsed, vec!["https://a.app", "https://b.app"]);
    }
}
