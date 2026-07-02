//! The provisioning artifact: everything the trader fleet, the reuse services, and
//! the operator need to point at the simulated market. Written once by the
//! provisioner and re-read on every run so re-provisioning (e.g. after a devnet
//! reset) is idempotent.

use serde::{Deserialize, Serialize};

use crate::error::SimError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraderEntry {
    pub keypair_path: String,
    pub pubkey: String,
    pub persona: String,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub keypair_path: String,
    pub pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimArtifact {
    pub market: String,
    pub market_seed_pubkey: String,
    pub oracle: String,
    /// `None` on a clearing-only (Phase A) market.
    pub collateral_mint: Option<String>,
    /// The vault's SPL token account (owned by the vault-authority PDA), Phase B only.
    pub vault_token_account: Option<String>,
    pub keeper: AgentEntry,
    pub liquidator: AgentEntry,
    pub market_makers: Vec<AgentEntry>,
    pub traders: Vec<TraderEntry>,
    /// Number of slab shards the market was provisioned with (Stage A). Traders route
    /// deterministically to one shard by `shard_for_trader(trader, num_slab_shards)`, and
    /// the keeper fans out across all shards. Defaults to 1 for pre-sharding artifacts.
    #[serde(default = "default_num_slab_shards")]
    pub num_slab_shards: u16,
}

fn default_num_slab_shards() -> u16 {
    1
}

impl SimArtifact {
    pub fn save(&self, path: &str) -> Result<(), SimError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &str) -> Result<Self, SimError> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_roundtrips_through_json() {
        let a = SimArtifact {
            market: "M".into(),
            market_seed_pubkey: "S".into(),
            oracle: "O".into(),
            collateral_mint: Some("C".into()),
            vault_token_account: Some("V".into()),
            keeper: AgentEntry {
                keypair_path: "k.json".into(),
                pubkey: "K".into(),
            },
            liquidator: AgentEntry {
                keypair_path: "l.json".into(),
                pubkey: "L".into(),
            },
            market_makers: vec![AgentEntry {
                keypair_path: "mm.json".into(),
                pubkey: "MM".into(),
            }],
            traders: vec![TraderEntry {
                keypair_path: "t.json".into(),
                pubkey: "T".into(),
                persona: "noise".into(),
                seed: 7,
            }],
            num_slab_shards: 8,
        };
        let json = serde_json::to_string(&a).unwrap();
        let b: SimArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(b.market, "M");
        assert_eq!(b.collateral_mint.as_deref(), Some("C"));
        assert_eq!(b.traders[0].seed, 7);
        assert_eq!(b.num_slab_shards, 8);
        // Pre-sharding artifacts (no field) default to 1 shard.
        let legacy = json.replace(",\"num_slab_shards\":8", "");
        let c: SimArtifact = serde_json::from_str(&legacy).unwrap();
        assert_eq!(c.num_slab_shards, 1);
    }
}
