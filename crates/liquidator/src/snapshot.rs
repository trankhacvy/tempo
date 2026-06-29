use std::collections::{HashMap, HashSet};

use solana_sdk::pubkey::Pubkey;

use tempo_sdk::accounts::{MarketView, PositionView};
use tempo_sdk::pda;

use crate::error::LiquidatorError;
use crate::LiqCtx;

/// One isolated-margin position with the data the engine's gate needs, priced at
/// the raw solvency mark.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub key: Pubkey,
    pub view: PositionView,
    pub market: Pubkey,
    pub oracle: Pubkey,
    pub mark: u64,
    pub maintenance_bps: u16,
}

/// One member of a cross-margin account, resolved for the combined-health gate. A
/// flat member (`size == 0`) carries only its `realized_pnl`; `market`/`oracle`/`mark`
/// are unused for it.
#[derive(Clone, Copy, Debug)]
pub struct CrossMember {
    pub position: Pubkey,
    pub size: i64,
    pub entry_price: u64,
    pub realized_pnl: i128,
    pub market: Pubkey,
    pub oracle: Pubkey,
    pub mark: u64,
    pub maintenance_bps: u16,
}

/// A resolved cross-margin account ready for the engine's combined-health gate.
#[derive(Clone, Debug)]
pub struct CrossAccount {
    pub balance: u64,
    pub members: Vec<CrossMember>,
}

/// Everything one scan reads from chain: the isolated candidates, the distinct
/// owners of cross positions (resolved separately), and the insurance balance.
pub struct Scan {
    pub isolated: Vec<Candidate>,
    pub cross_owners: Vec<Pubkey>,
    pub insurance: Option<u64>,
}

impl Scan {
    /// Read the underwater set for one scan. The `market_cache`/`price_cache` are
    /// owned by the caller and shared with [`resolve_cross`] so a market or oracle
    /// referenced by both an isolated position and a cross account is fetched once
    /// per scan, not once per owner.
    pub async fn load(
        ctx: &LiqCtx,
        now_ts: i64,
        market_cache: &mut HashMap<Pubkey, MarketView>,
        price_cache: &mut HashMap<Pubkey, u64>,
    ) -> Result<Scan, LiquidatorError> {
        let mut isolated = Vec::new();
        let mut cross_owners: Vec<Pubkey> = Vec::new();
        let mut seen_owners: HashSet<Pubkey> = HashSet::new();

        for market in &ctx.markets {
            let mv = market_view(ctx, market, market_cache).await?;
            let oracle = mv.oracle;
            let feed = mv.oracle_feed_id;
            let maintenance_bps = mv.maintenance_margin_bps;

            for (key, view) in ctx.source.live_positions(market).await? {
                if view.size == 0 {
                    continue;
                }
                if view.margin_mode == 1 {
                    if seen_owners.insert(view.owner) {
                        cross_owners.push(view.owner);
                    }
                    continue;
                }
                let Some(mark) = price_for(ctx, &oracle, &feed, now_ts, price_cache).await else {
                    continue; // stale / unpriceable oracle → skip this cycle
                };
                isolated.push(Candidate {
                    key,
                    view,
                    market: *market,
                    oracle,
                    mark,
                    maintenance_bps,
                });
            }
        }

        let insurance = match ctx.vault {
            Some(v) => ctx
                .client
                .fetch_vault(&v)
                .await?
                .map(|x| x.insurance_balance),
            None => None,
        };
        Ok(Scan {
            isolated,
            cross_owners,
            insurance,
        })
    }
}

/// Resolve one owner's cross-margin account: the shared balance plus every member
/// priced at its market's raw mark. Returns `None` when the owner has no group, no
/// collateral ledger, or a live member that cannot be priced this cycle (so the
/// account is left for a later scan rather than judged on partial data).
///
/// Takes owned caches (pre-seeded from the isolated scan) so this function can
/// be called from concurrent async tasks without shared mutable references.
pub async fn resolve_cross(
    ctx: &LiqCtx,
    owner: &Pubkey,
    now_ts: i64,
    mut market_cache: HashMap<Pubkey, MarketView>,
    mut price_cache: HashMap<Pubkey, u64>,
) -> Result<Option<CrossAccount>, LiquidatorError> {
    let margin_pda = pda::margin_account(owner).0;
    let Some(margin) = ctx.client.fetch_margin_account(&margin_pda).await? else {
        return Ok(None);
    };
    // Cross liquidation is a money-path operation; without a collateral mint there
    // is no mint-scoped ledger to read (CR-3).
    let Some(mint) = ctx.collateral_mint else {
        return Ok(None);
    };
    let collateral_pda = pda::user_collateral(owner, &mint).0;
    let Some(collateral) = ctx.client.fetch_user_collateral(&collateral_pda).await? else {
        return Ok(None);
    };

    let mut members = Vec::with_capacity(margin.members.len());

    for member in &margin.members {
        let data = ctx.client.fetch_account_data(member).await?;
        let pv = PositionView::decode(&data)?;
        if pv.size == 0 {
            members.push(CrossMember {
                position: *member,
                size: 0,
                entry_price: pv.entry_price,
                realized_pnl: pv.realized_pnl,
                market: Pubkey::default(),
                oracle: Pubkey::default(),
                mark: 0,
                maintenance_bps: 0,
            });
            continue;
        }
        let mv = market_view(ctx, &pv.market, &mut market_cache).await?;
        let oracle = mv.oracle;
        let Some(mark) =
            price_for(ctx, &oracle, &mv.oracle_feed_id, now_ts, &mut price_cache).await
        else {
            return Ok(None); // a live member we cannot price → defer the account
        };
        members.push(CrossMember {
            position: *member,
            size: pv.size,
            entry_price: pv.entry_price,
            realized_pnl: pv.realized_pnl,
            market: pv.market,
            oracle,
            mark,
            maintenance_bps: mv.maintenance_margin_bps,
        });
    }

    Ok(Some(CrossAccount {
        balance: collateral.balance,
        members,
    }))
}

async fn market_view(
    ctx: &LiqCtx,
    market: &Pubkey,
    cache: &mut HashMap<Pubkey, MarketView>,
) -> Result<MarketView, LiquidatorError> {
    if let Some(mv) = cache.get(market) {
        return Ok(mv.clone());
    }
    let mv = ctx.client.fetch_market(market).await?;
    cache.insert(*market, mv.clone());
    Ok(mv)
}

async fn price_for(
    ctx: &LiqCtx,
    oracle: &Pubkey,
    feed_id: &[u8; 32],
    now_ts: i64,
    cache: &mut HashMap<Pubkey, u64>,
) -> Option<u64> {
    if let Some(p) = cache.get(oracle) {
        return Some(*p);
    }
    match ctx.client.fetch_oracle_price(oracle, feed_id, now_ts).await {
        Ok(p) => {
            cache.insert(*oracle, p);
            Some(p)
        }
        Err(e) => {
            tracing::debug!(oracle = %oracle, error = %e, "oracle unpriceable; skipping");
            None
        }
    }
}
