use async_trait::async_trait;
use serde::Serialize;
use tempo_sdk::Pubkey;
use utoipa::ToSchema;

use crate::error::ApiError;

/// One settled fill (event-derived). Populated by the future indexer.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FillRow {
    pub auction_id: u64,
    pub trader: String,
    pub side: u8,
    pub price: String,
    pub quantity: String,
    pub slot: u64,
}

/// One funding accrual point (event-derived). Populated by the future indexer.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FundingRow {
    pub funding_index: String,
    pub rate: String,
    pub slot: u64,
    pub ts: i64,
}

/// Source of historical, event-derived data (recent fills, funding history).
/// These cannot be reconstructed from current account state — they require the
/// Phase-2 Geyser→Postgres indexer. The trait is the seam: today the only impl
/// is [`NoHistory`]; a `PostgresHistory` lands behind it without touching the
/// router or the OpenAPI document.
#[async_trait]
pub trait HistorySource: Send + Sync {
    async fn recent_fills(&self, market: &Pubkey, limit: u32) -> Result<Vec<FillRow>, ApiError>;
    async fn funding_history(
        &self,
        market: &Pubkey,
        limit: u32,
    ) -> Result<Vec<FundingRow>, ApiError>;
}

/// The only `HistorySource` until the indexer ships. It advertises the gap with
/// a clear `501 Not Implemented` rather than faking data.
pub struct NoHistory;

#[async_trait]
impl HistorySource for NoHistory {
    async fn recent_fills(&self, _market: &Pubkey, _limit: u32) -> Result<Vec<FillRow>, ApiError> {
        Err(ApiError::NotIndexed(
            "fills require the indexer (not yet deployed)",
        ))
    }

    async fn funding_history(
        &self,
        _market: &Pubkey,
        _limit: u32,
    ) -> Result<Vec<FundingRow>, ApiError> {
        Err(ApiError::NotIndexed(
            "funding history requires the indexer (not yet deployed)",
        ))
    }
}
