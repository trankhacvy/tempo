use solana_sdk::pubkey::Pubkey;
use tempo_common::env_parse;

use crate::error::LiquidatorError;

/// Liquidator configuration: the shared `tempo_common::Config` plus
/// liquidator-specific knobs from `TEMPO_*` env. Cross accounts span markets, so
/// the scan takes a market *list* (`TEMPO_MARKETS="pk1,pk2"`, falling back to the
/// single `TEMPO_MARKET`).
#[derive(Debug, Clone)]
pub struct LiquidatorConfig {
    pub common: tempo_common::Config,
    pub markets: Vec<Pubkey>,
    pub poll_interval_ms: u64,
    pub scan_concurrency: usize,
    pub stale_scan_secs: u64,
    pub health_addr: String,
}

fn parse_markets(list: &str) -> Result<Vec<Pubkey>, LiquidatorError> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<Pubkey>()
                .map_err(|_| LiquidatorError::Config(format!("invalid market pubkey: {s}")))
        })
        .collect()
}

impl LiquidatorConfig {
    pub fn load() -> Result<Self, LiquidatorError> {
        let common = tempo_common::Config::load().map_err(LiquidatorError::Common)?;
        let raw = std::env::var("TEMPO_MARKETS")
            .ok()
            .or_else(|| common.market.clone())
            .ok_or_else(|| {
                LiquidatorError::Config("TEMPO_MARKETS or TEMPO_MARKET is required".into())
            })?;
        let markets = parse_markets(&raw)?;
        if markets.is_empty() {
            return Err(LiquidatorError::Config(
                "no markets configured (TEMPO_MARKETS/TEMPO_MARKET empty)".into(),
            ));
        }
        Ok(Self {
            common,
            markets,
            poll_interval_ms: env_parse("TEMPO_LIQ_POLL_MS", 2000),
            scan_concurrency: env_parse("TEMPO_LIQ_CONCURRENCY", 8),
            stale_scan_secs: env_parse("TEMPO_LIQ_STALE_SCAN_SECS", 30),
            health_addr: std::env::var("TEMPO_HEALTH_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8081".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_comma_list_trimming_blanks() {
        let a = Pubkey::new_unique().to_string();
        let b = Pubkey::new_unique().to_string();
        let got = parse_markets(&format!(" {a}, {b} ,")).unwrap();
        assert_eq!(got, vec![a.parse().unwrap(), b.parse().unwrap()]);
    }

    #[test]
    fn rejects_a_bad_pubkey() {
        assert!(parse_markets("not-a-key").is_err());
    }

    #[test]
    fn empty_list_is_empty() {
        assert!(parse_markets("  , ,").unwrap().is_empty());
    }
}
