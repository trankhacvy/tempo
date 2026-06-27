use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;
use solana_commitment_config::CommitmentConfig;

use crate::error::CommonError;

fn default_commitment() -> String {
    "confirmed".to_string()
}

fn default_metrics_addr() -> String {
    "127.0.0.1:9100".to_string()
}

/// Parse an env var into `T`, falling back to `default` on absence or parse
/// failure. Shared by all service config modules to avoid copy-paste.
pub fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Service configuration, loaded from an optional `tempo.toml` overlaid with
/// `TEMPO_*` environment variables (env wins). Field names map to the existing
/// bot env vars: `rpc_url` ← `TEMPO_RPC_URL`, `keypair` ← `TEMPO_KEYPAIR`, etc.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub rpc_url: String,
    #[serde(default = "default_commitment")]
    pub commitment: String,
    #[serde(default)]
    pub keypair: Option<String>,
    #[serde(default)]
    pub market: Option<String>,
    #[serde(default)]
    pub collateral_mint: Option<String>,
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,
    /// Compute-unit price in micro-lamports prepended to every transaction.
    /// `0` omits the price instruction (devnet default). Set to ≥1000 on mainnet.
    #[serde(default)]
    pub priority_fee_micro_lamports: u64,
}

impl Config {
    pub fn load() -> Result<Self, CommonError> {
        Figment::new()
            .merge(Toml::file("tempo.toml"))
            .merge(Env::prefixed("TEMPO_"))
            .extract()
            .map_err(|e| CommonError::Config(e.to_string()))
    }

    pub fn commitment_config(&self) -> CommitmentConfig {
        match self.commitment.as_str() {
            "processed" => CommitmentConfig::processed(),
            "finalized" => CommitmentConfig::finalized(),
            _ => CommitmentConfig::confirmed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(commitment: &str) -> Config {
        Config {
            rpc_url: "https://rpc.example/key".to_string(),
            commitment: commitment.to_string(),
            keypair: None,
            market: None,
            collateral_mint: None,
            metrics_addr: default_metrics_addr(),
            priority_fee_micro_lamports: 0,
        }
    }

    #[test]
    fn test_commitment_mapping() {
        assert_eq!(
            cfg_with("processed").commitment_config(),
            CommitmentConfig::processed()
        );
        assert_eq!(
            cfg_with("finalized").commitment_config(),
            CommitmentConfig::finalized()
        );
        assert_eq!(
            cfg_with("confirmed").commitment_config(),
            CommitmentConfig::confirmed()
        );
        assert_eq!(
            cfg_with("garbage").commitment_config(),
            CommitmentConfig::confirmed()
        );
    }
}
