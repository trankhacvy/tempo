use tempo_common::env_parse;

use crate::error::MmError;
use crate::strategy::MmStrategyConfig;

/// Market-maker configuration: the shared `tempo_common::Config` plus the
/// strategy + cadence knobs, read from `TEMPO_MM_*` env with production-safe
/// defaults.
#[derive(Debug, Clone)]
pub struct MmConfig {
    pub common: tempo_common::Config,
    pub strategy: MmStrategyConfig,
    pub poll_ms: u64,
    pub health_addr: String,
    pub stale_quote_windows: u64,
}

impl MmConfig {
    pub fn load() -> Result<Self, MmError> {
        let common = tempo_common::Config::load().map_err(MmError::Common)?;
        let strategy = MmStrategyConfig {
            levels: env_parse::<u8>("TEMPO_MM_LEVELS", 3).clamp(1, 8),
            inner_spread_ticks: env_parse("TEMPO_MM_INNER_SPREAD_TICKS", 1),
            tick_step: env_parse("TEMPO_MM_TICK_STEP", 1),
            base_size: env_parse("TEMPO_MM_BASE_SIZE", 100),
            size_growth_num: env_parse("TEMPO_MM_SIZE_GROWTH_NUM", 1),
            size_growth_den: env_parse::<u32>("TEMPO_MM_SIZE_GROWTH_DEN", 1).max(1),
            max_inventory: env_parse("TEMPO_MM_MAX_INVENTORY", 10_000),
            skew_ticks_max: env_parse("TEMPO_MM_SKEW_TICKS_MAX", 2),
        };
        Ok(Self {
            common,
            strategy,
            poll_ms: env_parse("TEMPO_MM_POLL_MS", 800),
            health_addr: std::env::var("TEMPO_MM_HEALTH_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8081".to_string()),
            stale_quote_windows: env_parse("TEMPO_MM_STALE_QUOTE_WINDOWS", 3),
        })
    }

    pub fn expiry_slots(&self) -> u64 {
        env_parse("TEMPO_MM_EXPIRY_SLOTS", 0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn levels_are_clamped_to_valid_range() {
        assert_eq!(12u8.clamp(1, 8), 8);
        assert_eq!(0u8.clamp(1, 8), 1);
    }
}
