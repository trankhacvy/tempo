use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use tempo_sdk::ix::{self, CrossLeg, LiquidateCrossParams, LiquidateParams};
use tempo_sdk::{benign, pda};

use crate::engine::LiqAction;
use crate::LiqCtx;

/// Send one liquidation. A `NotLiquidatable` / "already processed" / wrong-phase
/// race is expected (another liquidator won, or the price ticked back) and counts
/// as benign, not an error — the program is the final gate.
async fn fire(ctx: &LiqCtx, instruction: Instruction, kind: &'static str) {
    match ctx.client.send(&ctx.liquidator, &[instruction]).await {
        Ok(sig) => {
            tracing::info!(%sig, kind, "liquidation landed");
            metrics::counter!("liquidator_fired_total", "kind" => kind, "result" => "ok")
                .increment(1);
        }
        Err(e) if benign(&e) => {
            metrics::counter!("liquidator_fired_total", "kind" => kind, "result" => "benign")
                .increment(1);
        }
        Err(e) => {
            tracing::warn!(kind, error = %e, "liquidation send failed");
            metrics::counter!("liquidator_fired_total", "kind" => kind, "result" => "error")
                .increment(1);
        }
    }
}

/// Fire an isolated `liquidate` for one underwater position.
pub async fn liquidate_isolated(ctx: &LiqCtx, action: &LiqAction) {
    let LiqAction::Isolated {
        position,
        owner,
        market,
        oracle,
    } = action
    else {
        return;
    };
    let Some(vault) = ctx.vault else {
        tracing::warn!("isolated liquidation skipped: no vault configured");
        return;
    };
    let Some(mint) = ctx.collateral_mint else {
        tracing::warn!("isolated liquidation skipped: no collateral mint configured");
        return;
    };
    let params = LiquidateParams {
        liquidator: ctx.liquidator.pubkey(),
        market: *market,
        oracle: *oracle,
        position: *position,
        user_collateral: pda::user_collateral(owner, &mint).0,
        vault,
        liquidator_collateral: ctx.liquidator_collateral,
    };
    fire(ctx, ix::liquidate(&params), "isolated").await;
}

/// Fire a `liquidate_cross` for one combined-unhealthy account (closes one member;
/// repeated scans wind it down in bounded steps).
pub async fn liquidate_cross(ctx: &LiqCtx, owner: &Pubkey, legs: &[CrossLeg]) {
    let Some(vault) = ctx.vault else {
        tracing::warn!("cross liquidation skipped: no vault configured");
        return;
    };
    let Some(mint) = ctx.collateral_mint else {
        tracing::warn!("cross liquidation skipped: no collateral mint configured");
        return;
    };
    let params = LiquidateCrossParams {
        liquidator: ctx.liquidator.pubkey(),
        margin_account: pda::margin_account(owner).0,
        user_collateral: pda::user_collateral(owner, &mint).0,
        vault,
        liquidator_collateral: ctx.liquidator_collateral,
    };
    fire(ctx, ix::liquidate_cross(&params, legs), "cross").await;
}
