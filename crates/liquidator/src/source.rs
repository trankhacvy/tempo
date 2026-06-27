use std::sync::Arc;

use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;

use tempo_sdk::accounts::PositionView;
use tempo_sdk::{SdkError, TempoClient};

/// Where the liquidator learns which positions exist. The chain-scan impl ships
/// now; an indexer-backed impl (`crates/indexer`) drops in unchanged behind this
/// trait when the indexer lands (build-plan §2.9 / Decision D1).
#[async_trait(?Send)]
pub trait PositionSource: Send + Sync {
    async fn live_positions(
        &self,
        market: &Pubkey,
    ) -> Result<Vec<(Pubkey, PositionView)>, SdkError>;
}

/// Bounded `getProgramAccounts` scan over one market's `Position` accounts (the
/// same memcmp scan the keeper uses for maker quotes). Correct but heavier than the
/// future indexer, so it is isolated behind [`PositionSource`].
pub struct ChainScan {
    pub client: Arc<TempoClient>,
}

impl ChainScan {
    pub fn new(client: Arc<TempoClient>) -> Self {
        Self { client }
    }
}

#[async_trait(?Send)]
impl PositionSource for ChainScan {
    async fn live_positions(
        &self,
        market: &Pubkey,
    ) -> Result<Vec<(Pubkey, PositionView)>, SdkError> {
        self.client.fetch_positions(market).await
    }
}

/// A canned [`PositionSource`] for tests — returns a fixed list, no RPC.
#[cfg(test)]
pub struct MockSource {
    pub rows: Vec<(Pubkey, PositionView)>,
}

#[cfg(test)]
#[async_trait(?Send)]
impl PositionSource for MockSource {
    async fn live_positions(
        &self,
        _market: &Pubkey,
    ) -> Result<Vec<(Pubkey, PositionView)>, SdkError> {
        Ok(self.rows.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_source_returns_canned_rows() {
        let key = Pubkey::new_unique();
        let view = PositionView {
            owner: Pubkey::new_unique(),
            market: Pubkey::new_unique(),
            size: 5,
            entry_price: 100,
            collateral: 50,
            realized_pnl: 0,
            margin_mode: 0,
        };
        let src = MockSource {
            rows: vec![(key, view)],
        };
        let got = src.live_positions(&Pubkey::new_unique()).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, key);
    }
}
