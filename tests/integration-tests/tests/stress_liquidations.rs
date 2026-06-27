//! Heavy-load randomized liquidation stress (the risk path).
//!
//! Opens long/short pairs with thin collateral, then random-walks the oracle and
//! liquidates whatever falls below maintenance (longs on dips, shorts on pumps),
//! including deep moves that produce bad debt socialized to the winning side
//! (ADL). It asserts the strong solvency invariant after EVERY liquidation
//! attempt:
//!
//!   `Σ user_collateral.balance + insurance == vault token holdings`
//!
//! This holds exactly at all times because a liquidation only moves value between
//! the loser's balance, the liquidator's balance, and insurance (which conserve),
//! while the winner's gain and any socialized loss stay *unrealized* (off-balance)
//! until that winner later settles. So liquidation — modest or bad-debt — never
//! creates or destroys backing. Deterministic (seeded) for exact reproduction.

use tempo_integration_tests::*;

const MAINT_BPS: u16 = 1000; // 10%
const QTY: u64 = 10;
const ENTRY: u64 = 30;
// A taker SELL reserves worst-case margin at submit (window top = tick 127 ≈ 128
// here): 10·128·10% = 128 (missing-features §1.1), released at settle down to the
// actual 30 locked. Fund above that; the free buffer does not protect an isolated
// position (liquidation prices off the position's locked collateral), so the
// liquidation dynamics are unchanged.
const DEPOSIT: u64 = 200;
const PAIRS: usize = 3; // 6 traders
const STEPS: u64 = 60;

#[test]
fn stress_liquidations_conserve_under_random_oracle_walk() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
    let pdas = ctx.init_market_with_oracle(1, 128, 32, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // The liquidator just needs a ledger to receive penalties into.
    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Trader pool: PAIRS of (long, short), each posting DEPOSIT.
    let n = PAIRS * 2;
    let mut traders = Vec::with_capacity(n);
    for _ in 0..n {
        let t = ctx.new_funded_signer();
        ctx.init_collateral(&t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, DEPOSIT);
        ctx.deposit(&t, &vault_ta, &ta, DEPOSIT);
        ctx.init_position(&pdas, &t);
        traders.push(t);
    }
    let vault_tokens = ctx.token_balance(&vault_ta);
    assert_eq!(vault_tokens, DEPOSIT * n as u64);

    // Strong solvency invariant over every ledger (traders + liquidator) + insurance.
    macro_rules! assert_solvent {
        () => {{
            let mut sum =
                ctx.vault().insurance_balance + ctx.user_collateral(&liquidator.pubkey()).balance;
            for t in traders.iter() {
                sum += ctx.user_collateral(&t.pubkey()).balance;
            }
            assert_eq!(sum, vault_tokens, "Σ balances + insurance == vault tokens");
        }};
    }

    // --- Open every pair: long maker-buy (quote book) / short taker-sell, 10 @ 30. ---
    let mut maker_idx = Vec::new(); // longs (makers via quote book)
    let mut taker_orders = Vec::new(); // shorts (takers via submit_order)
    for k in 0..PAIRS {
        ctx.post_maker_order(&pdas, &traders[2 * k], SIDE_BUY, ENTRY, QTY);
        let sid = ctx.submit_order(&pdas, &traders[2 * k + 1], SIDE_SELL, ENTRY, QTY);
        maker_idx.push(2 * k);
        taker_orders.push((sid, 2 * k + 1));
    }
    ctx.process_chunk(&pdas, 0, 32);
    for &mi in &maker_idx {
        ctx.process_maker_quote(&pdas, &traders[mi].pubkey());
    }
    ctx.finalize_clear(&pdas);
    for &mi in &maker_idx {
        ctx.settle_maker_quote(&pdas, &traders[mi].pubkey());
    }
    for &(oid, ti) in &taker_orders {
        ctx.settle_fill_with_margin(&pdas, oid, &traders[ti].pubkey());
    }
    assert_eq!(ctx.market(&pdas).oi_long, (PAIRS as u128) * QTY as u128);
    assert_solvent!();

    // `open[i]` tracks which positions are still live (liquidation closes them).
    let mut open = vec![true; n];

    let mut seed: u64 = 0xfeed_face_dead_beef;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };

    let mut price: i64 = ENTRY as i64;
    let mut clock = 1_700_000_000i64;
    let mut any_bad_debt = false;
    let mut any_liquidation = false;

    for step in 0..STEPS {
        if step == 0 {
            // Flash crash: gap straight past the longs' solvency window (a long
            // 10@30 with 30 margin is underwater at p<30 and insolvent at p<=27),
            // so the longs liquidate with bad debt that socializes to the shorts —
            // exercising the ADL path under the stress harness. Subsequent steps
            // random-walk to liquidate the shorts as the price recovers.
            price = 18;
        } else {
            // Random-walk the oracle in [12, 48]: dips put longs underwater, pumps
            // put shorts underwater.
            let delta = (next() % 13) as i64 - 6; // -6..=+6
            price = (price + delta).clamp(12, 48);
        }
        clock += 1;
        ctx.set_clock_ts(clock);
        ctx.set_oracle(&oracle, price, -8);

        for i in 0..n {
            if !open[i] {
                continue;
            }
            match ctx.try_liquidate(&pdas, &oracle, &liquidator, &traders[i].pubkey()) {
                Ok(_) => {
                    open[i] = false;
                    any_liquidation = true;
                    assert_eq!(
                        ctx.position(&ctx.position_pda(&pdas, &traders[i].pubkey()).0)
                            .size,
                        0,
                        "liquidated position is flat"
                    );
                    // Detect a bad-debt close: the social index moved on some side.
                    let m = ctx.market(&pdas);
                    if m.social_loss_index_long != 0 || m.social_loss_index_short != 0 {
                        any_bad_debt = true;
                    }
                }
                Err(_) => { /* healthy or flat — a clean no-op revert */ }
            }
            // Solvency holds after every attempt, success or revert.
            assert_solvent!();
        }
    }

    // The walk must have actually exercised the path (not a vacuous pass).
    assert!(
        any_liquidation,
        "the oracle walk should have liquidated someone"
    );
    assert!(
        any_bad_debt,
        "the extreme moves should have produced socialized bad debt"
    );
    assert_solvent!();
    // Insurance is a u64 — it never went negative (no underflow panic above).
}
