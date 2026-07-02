use tempo_common::env_parse;
use tempo_sdk::consts::MAX_ORDERS_PER_TRADER;

use crate::error::SimError;
use crate::persona::Persona;
use crate::strategy::TraderConfig;

/// Devnet Pyth SOL/USD `PriceUpdateV2` account (the market's bound oracle).
pub const DEVNET_SOL_USD_ORACLE: &str = "7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE";

/// One trader process's configuration: the shared `tempo_common::Config` plus the
/// strategy + cadence knobs, read from `TEMPO_SIM_*` env with devnet-safe defaults.
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub common: tempo_common::Config,
    pub persona: Persona,
    pub seed: u64,
    pub poll_ms: u64,
    pub base_size: u64,
    pub aggression_ticks: u16,
    pub inner_spread_ticks: u16,
    pub max_orders: u8,
    /// `Some(0)`/`Some(1)` forces every order to buy/sell (see `TraderConfig::force_side`).
    pub force_side: Option<u8>,
    pub health_addr: String,
    pub stale_windows: u64,
}

impl SimConfig {
    pub fn load() -> Result<Self, SimError> {
        let common = tempo_common::Config::load().map_err(SimError::Common)?;
        let max_orders =
            env_parse::<u8>("TEMPO_SIM_MAX_ORDERS", 3).clamp(1, MAX_ORDERS_PER_TRADER as u8);
        Ok(Self {
            common,
            persona: Persona::parse(
                &std::env::var("TEMPO_SIM_PERSONA").unwrap_or_else(|_| "noise".to_string()),
            ),
            seed: env_parse("TEMPO_SIM_SEED", 1),
            poll_ms: env_parse("TEMPO_SIM_POLL_MS", 800),
            base_size: env_parse("TEMPO_SIM_BASE_SIZE", 5),
            aggression_ticks: env_parse("TEMPO_SIM_AGGRESSION_TICKS", 2),
            inner_spread_ticks: env_parse("TEMPO_SIM_INNER_SPREAD_TICKS", 1),
            max_orders,
            // TEMPO_SIM_FORCE_SIDE = "buy"/"0" or "sell"/"1"; unset = persona-driven.
            force_side: match std::env::var("TEMPO_SIM_FORCE_SIDE").ok().as_deref() {
                Some("buy") | Some("0") => Some(0),
                Some("sell") | Some("1") => Some(1),
                _ => None,
            },
            health_addr: std::env::var("TEMPO_SIM_HEALTH_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8083".to_string()),
            stale_windows: env_parse("TEMPO_SIM_STALE_WINDOWS", 5),
        })
    }

    pub fn trader_config(&self) -> TraderConfig {
        TraderConfig {
            persona: self.persona,
            max_orders: self.max_orders,
            base_size: self.base_size,
            aggression_ticks: self.aggression_ticks,
            inner_spread_ticks: self.inner_spread_ticks,
            force_side: self.force_side,
        }
    }
}

/// Provisioner / orchestrator configuration (`TEMPO_SIM_*`), used by the one-shot
/// provisioner and the local single-process orchestrator. The money-path knobs
/// (`maint_bps > 0`, deposit) select Phase B; leaving `maint_bps == 0` provisions a
/// clearing-only Phase-A market.
#[derive(Debug, Clone)]
pub struct ProvisionConfig {
    pub rpc_url: String,
    pub commitment: String,
    pub master_keypair: String,
    pub keys_dir: String,
    pub artifact_path: String,
    pub oracle: String,
    // market params
    pub tick_size: u64,
    pub num_ticks: u32,
    pub cap: u32,
    pub maint_bps: u16,
    pub initial_bps: u16,
    pub penalty_bps: u16,
    pub max_price_move_bps_per_slot: u16,
    pub soft_stale_slots: u64,
    // money path
    pub collateral_decimals: u8,
    pub deposit_amount: u64,
    // fleet
    pub num_traders: u32,
    pub num_mm: u32,
    pub fund_lamports: u64,
}

impl ProvisionConfig {
    pub fn load() -> Result<Self, SimError> {
        let common = tempo_common::Config::load().map_err(SimError::Common)?;
        let maint_bps = env_parse::<u16>("TEMPO_SIM_MAINT_BPS", 0);
        // initial_margin must be >= maintenance; default to maintenance when unset.
        let initial_bps = {
            let v = env_parse::<u16>("TEMPO_SIM_INITIAL_BPS", maint_bps);
            if maint_bps == 0 {
                0
            } else {
                v.max(maint_bps)
            }
        };
        let penalty_bps = if maint_bps == 0 {
            0
        } else {
            env_parse::<u16>("TEMPO_SIM_PENALTY_BPS", 100)
        };
        Ok(Self {
            rpc_url: common.rpc_url.clone(),
            commitment: common.commitment.clone(),
            master_keypair: std::env::var("TEMPO_SIM_MASTER_KEYPAIR")
                .or_else(|_| std::env::var("TEMPO_KEYPAIR"))
                .map_err(|_| SimError::Config("TEMPO_SIM_MASTER_KEYPAIR is required".into()))?,
            keys_dir: std::env::var("TEMPO_SIM_KEYS_DIR").unwrap_or_else(|_| "./keys".to_string()),
            artifact_path: std::env::var("TEMPO_SIM_ARTIFACT")
                .unwrap_or_else(|_| "./sim-artifact.json".to_string()),
            oracle: std::env::var("TEMPO_SIM_ORACLE")
                .unwrap_or_else(|_| DEVNET_SOL_USD_ORACLE.to_string()),
            tick_size: env_parse("TEMPO_SIM_TICK_SIZE", 10_000_000),
            num_ticks: env_parse::<u32>("TEMPO_SIM_NUM_TICKS", 256).clamp(2, 256),
            // Per-shard order cap. Must stay within the on-chain single-`CreateAccount`
            // ceiling `MAX_ORDERS_PER_AUCTION_CAP`, now 90 at ORDER_LEN=104 (Stage B). A cap
            // above 90 is rejected by `initialize_market`, so clamp the ceiling to 90 to fail
            // fast in config rather than at provisioning.
            cap: env_parse::<u32>("TEMPO_SIM_CAP", 90).clamp(1, 90),
            maint_bps,
            initial_bps,
            penalty_bps,
            max_price_move_bps_per_slot: env_parse("TEMPO_SIM_MAX_PRICE_MOVE_BPS", 0),
            soft_stale_slots: env_parse("TEMPO_SIM_SOFT_STALE_SLOTS", 0),
            collateral_decimals: env_parse("TEMPO_SIM_COLLATERAL_DECIMALS", 6),
            deposit_amount: env_parse("TEMPO_SIM_DEPOSIT", 1_000_000_000_000_000),
            num_traders: env_parse::<u32>("TEMPO_SIM_NUM_TRADERS", 12).clamp(1, 200),
            num_mm: env_parse::<u32>("TEMPO_SIM_NUM_MM", 2).clamp(1, 50),
            fund_lamports: env_parse("TEMPO_SIM_FUND_LAMPORTS", 50_000_000),
        })
    }

    /// True when the market has a money path (positions, margin, liquidations).
    pub fn is_money_market(&self) -> bool {
        self.maint_bps > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_market_flag_tracks_maintenance() {
        let mut c = ProvisionConfig {
            rpc_url: String::new(),
            commitment: "confirmed".into(),
            master_keypair: String::new(),
            keys_dir: String::new(),
            artifact_path: String::new(),
            oracle: String::new(),
            tick_size: 10,
            num_ticks: 64,
            cap: 64,
            maint_bps: 0,
            initial_bps: 0,
            penalty_bps: 0,
            max_price_move_bps_per_slot: 0,
            soft_stale_slots: 0,
            collateral_decimals: 6,
            deposit_amount: 0,
            num_traders: 1,
            num_mm: 1,
            fund_lamports: 0,
        };
        assert!(!c.is_money_market());
        c.maint_bps = 500;
        assert!(c.is_money_market());
    }
}
