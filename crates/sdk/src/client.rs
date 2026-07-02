use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, MemcmpEncodedBytes, RpcFilterType};
use solana_client::rpc_response::UiAccountEncoding;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signature::Signature;

use tempo_common::{RpcPool, TxSender, DEFAULT_CU_LIMIT};

use crate::accounts::{
    ClearingResultView, HistogramView, MakerQuoteView, MarginAccountView, MarketView, PositionView,
    UserCollateralView, VaultView,
};
use crate::error::SdkError;
use crate::ids::{PYTH_RECEIVER_ID, TEMPO_PROGRAM_ID};

/// High-level read + send facade over an [`RpcPool`]. The keeper, market-maker,
/// and liquidator build on this rather than touching the RPC layer directly.
pub struct TempoClient {
    pool: RpcPool,
    priority_fee_micro_lamports: u64,
}

impl TempoClient {
    pub fn new(pool: RpcPool, priority_fee_micro_lamports: u64) -> Self {
        Self {
            pool,
            priority_fee_micro_lamports,
        }
    }

    pub fn pool(&self) -> &RpcPool {
        &self.pool
    }

    /// Fetch and decode the per-market control block.
    pub async fn fetch_market(&self, market: &Pubkey) -> Result<MarketView, SdkError> {
        let data = self.fetch_account_data(market).await?;
        MarketView::decode(&data)
    }

    /// Raw account data over the pool (used by the hand-rolled decoders).
    pub async fn fetch_account_data(&self, key: &Pubkey) -> Result<Vec<u8>, SdkError> {
        let k = *key;
        let account = self
            .pool
            .call(8, async |rpc| rpc.get_account(&k).await)
            .await?;
        Ok(account.data)
    }

    /// Raw account data, or `None` when the account does not exist yet (e.g. a
    /// `ClearingResult` that has never been written).
    pub async fn fetch_account_data_opt(&self, key: &Pubkey) -> Result<Option<Vec<u8>>, SdkError> {
        let k = *key;
        let account = self
            .pool
            .call(8, async |rpc| {
                rpc.get_account_with_commitment(&k, rpc.commitment()).await
            })
            .await?;
        Ok(account.value.map(|a| a.data))
    }

    /// The current confirmed slot.
    pub async fn current_slot(&self) -> Result<u64, SdkError> {
        Ok(self.pool.call(8, async |rpc| rpc.get_slot().await).await?)
    }

    /// Enumerate this market's `MakerQuote` accounts (disc 8, market at offset 34).
    /// Bounded single-market scan; the Phase 2 indexer removes it.
    pub async fn fetch_maker_quotes(
        &self,
        market: &Pubkey,
    ) -> Result<Vec<(Pubkey, MakerQuoteView)>, SdkError> {
        self.fetch_by_disc_and_market(8, market, MakerQuoteView::decode)
            .await
    }

    /// Fetch and decode the auction histogram (the four dual-auction bucket
    /// arrays the UI draws the cross on).
    pub async fn fetch_histogram(&self, histogram: &Pubkey) -> Result<HistogramView, SdkError> {
        let data = self.fetch_account_data(histogram).await?;
        HistogramView::decode(&data)
    }

    /// Fetch and decode the published `ClearingResult`, or `None` before the
    /// market's first `finalize_clear` (the account does not exist yet).
    pub async fn fetch_clearing(
        &self,
        clearing: &Pubkey,
    ) -> Result<Option<ClearingResultView>, SdkError> {
        self.fetch_decode_opt(clearing, ClearingResultView::decode)
            .await
    }

    /// Fetch and decode one trader's collateral ledger, or `None` when it has
    /// never been initialized.
    pub async fn fetch_user_collateral(
        &self,
        user_collateral: &Pubkey,
    ) -> Result<Option<UserCollateralView>, SdkError> {
        self.fetch_decode_opt(user_collateral, UserCollateralView::decode)
            .await
    }

    /// Enumerate this market's `Position` accounts via a `getProgramAccounts`
    /// memcmp on the discriminator (`5` at offset 0) and the `market` field
    /// (offset 34, the same relative place as `MakerQuote.market`). A bounded
    /// single-market scan; the Phase 2 indexer removes it.
    pub async fn fetch_positions(
        &self,
        market: &Pubkey,
    ) -> Result<Vec<(Pubkey, PositionView)>, SdkError> {
        self.fetch_by_disc_and_market(5, market, PositionView::decode)
            .await
    }

    /// Fetch and decode the global `Vault` for insurance health, or `None` when it
    /// has not been initialized.
    pub async fn fetch_vault(&self, vault: &Pubkey) -> Result<Option<VaultView>, SdkError> {
        self.fetch_decode_opt(vault, VaultView::decode).await
    }

    /// Fetch and decode one owner's cross-margin `MarginAccount`, or `None` when the
    /// owner holds no cross group.
    pub async fn fetch_margin_account(
        &self,
        margin_account: &Pubkey,
    ) -> Result<Option<MarginAccountView>, SdkError> {
        self.fetch_decode_opt(margin_account, MarginAccountView::decode)
            .await
    }

    /// Fetch an optional account and decode it with `decode_fn`. Returns `None`
    /// when the account does not exist yet.
    async fn fetch_decode_opt<T>(
        &self,
        key: &Pubkey,
        decode_fn: fn(&[u8]) -> Result<T, SdkError>,
    ) -> Result<Option<T>, SdkError> {
        match self.fetch_account_data_opt(key).await? {
            Some(data) => Ok(Some(decode_fn(&data)?)),
            None => Ok(None),
        }
    }

    /// Enumerate program accounts filtered by discriminator byte at offset 0 and
    /// market pubkey at offset 34 (the shared layout of `Position` and `MakerQuote`).
    #[allow(deprecated)] // get_program_accounts_with_config: the UI-account replacement
                         // returns base64 strings we'd only decode straight back to bytes.
    async fn fetch_by_disc_and_market<T>(
        &self,
        disc: u8,
        market: &Pubkey,
        decode_fn: fn(&[u8]) -> Result<T, SdkError>,
    ) -> Result<Vec<(Pubkey, T)>, SdkError> {
        // Request base64 account data encoding — many RPC providers (including Helius)
        // reject getProgramAccounts when the returned account data would be encoded as
        // base58 but exceeds ~128 bytes (MakerQuote is ~450 bytes, Position ~138 bytes).
        // Also use base64 for the memcmp filter bytes for the same reason.
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![
                RpcFilterType::Memcmp(Memcmp::new(
                    0,
                    MemcmpEncodedBytes::Base64(B64.encode([disc])),
                )),
                RpcFilterType::Memcmp(Memcmp::new(
                    34,
                    MemcmpEncodedBytes::Base64(B64.encode(market.to_bytes())),
                )),
            ]),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..Default::default()
            },
            ..Default::default()
        };
        let accounts = self
            .pool
            .call(6, async |rpc| {
                rpc.get_program_accounts_with_config(&TEMPO_PROGRAM_ID, config.clone())
                    .await
            })
            .await?;
        let mut out = Vec::with_capacity(accounts.len());
        for (pubkey, account) in accounts {
            if let Ok(view) = decode_fn(&account.data) {
                out.push((pubkey, view));
            }
        }
        Ok(out)
    }

    /// Fetch a Pyth `PriceUpdateV2`, verify receiver-program ownership, and resolve
    /// the raw, confidence-checked solvency price (1e8) the on-chain solvency path
    /// reads. `now_ts` is the caller's wall clock (seconds). Errors on a missing,
    /// foreign-owned, stale, or too-uncertain oracle.
    pub async fn fetch_oracle_price(
        &self,
        oracle: &Pubkey,
        feed_id: &[u8; 32],
        now_ts: i64,
    ) -> Result<u64, SdkError> {
        let k = *oracle;
        let account = self
            .pool
            .call(8, async |rpc| rpc.get_account(&k).await)
            .await?;
        if account.owner != PYTH_RECEIVER_ID {
            return Err(SdkError::Decode(
                "oracle account not owned by the Pyth receiver program".to_string(),
            ));
        }
        let price = tempo_math::oracle::read_price(
            &account.data,
            feed_id,
            now_ts,
            tempo_math::oracle::MAX_AGE_SECS,
        )
        .map_err(|e| SdkError::Decode(e.to_string()))?;
        price
            .require_confidence(tempo_math::oracle::DEFAULT_MAX_CONF_BPS)
            .map_err(|e| SdkError::Decode(e.to_string()))?;
        Ok(price.price_1e8)
    }

    /// Send a transaction with compute-budget sizing, optional priority fee,
    /// and blockhash/conflict retry.
    pub async fn send(&self, payer: &Keypair, ixs: &[Instruction]) -> Result<Signature, SdkError> {
        Ok(TxSender::new(&self.pool, self.priority_fee_micro_lamports)
            .send_with_conflict_retry(payer, ixs, DEFAULT_CU_LIMIT, 4)
            .await?)
    }

    /// Multi-signer send: `signers[0]` is the fee payer and every signer signs. Packs
    /// instructions from many distinct signers (e.g. one `submit_order` per trader) into
    /// one transaction for the high-volume batched submitter.
    pub async fn send_signed(
        &self,
        signers: &[&Keypair],
        ixs: &[Instruction],
    ) -> Result<Signature, SdkError> {
        Ok(TxSender::new(&self.pool, self.priority_fee_micro_lamports)
            .send_signed_with_conflict_retry(signers, ixs, DEFAULT_CU_LIMIT, 4)
            .await?)
    }

    /// Fire-and-forget multi-signer send: broadcast and return the signature WITHOUT
    /// waiting for confirmation (the high-volume flood's fast path — landing is verified
    /// out-of-band). A deterministic preflight rejection still surfaces as an error.
    pub async fn send_signed_no_confirm(
        &self,
        signers: &[&Keypair],
        ixs: &[Instruction],
    ) -> Result<Signature, SdkError> {
        Ok(TxSender::new(&self.pool, self.priority_fee_micro_lamports)
            .send_signed_no_confirm(signers, ixs, DEFAULT_CU_LIMIT)
            .await?)
    }
}
