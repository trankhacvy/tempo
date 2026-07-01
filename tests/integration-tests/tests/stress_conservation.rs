//! Heavy-load randomized end-to-end stress test (transaction-level load).
//!
//! Runs many auction rounds with deterministic-random prices and quantities,
//! opening and closing positions across a pool of traders, and asserts the
//! protocol's load-bearing invariants after EVERY round:
//!
//!   1. Solvency: `Σ user_collateral.balance + insurance == vault token holdings`.
//!   2. OI balance: `oi_long == oi_short`, and both return to 0 once a round's
//!      positions are fully closed.
//!   3. Zero-sum: end-to-end, total claims equal the total deposited (PnL nets).
//!
//! This is the sequence-level complement to the host property fuzzes (which hit
//! the pure math) and the targeted conservation tests (which hit one scenario):
//! it exercises `find_cross`, the settlement money path, OI tracking, the
//! winner/loser gate, and `start_auction` slab/histogram reuse under sustained
//! random load. Deterministic (seeded) so any failure reproduces exactly.

// The paired-trader loops index `traders` by position (parity selects long/short
// legs), so the range-index form reads clearer than an iterator here.
#![allow(clippy::needless_range_loop)]

use tempo_integration_tests::*;

const ROUNDS: u64 = 40;
const N: usize = 6; // traders, paired long/short

/// `Σ balances + insurance == vault tokens` over the whole trader pool.
macro_rules! assert_conserved {
    ($ctx:expr, $traders:expr, $vault_ta:expr) => {{
        let mut sum = $ctx.vault().insurance_balance;
        for t in $traders.iter() {
            sum += $ctx.user_collateral(&t.pubkey()).balance;
        }
        assert_eq!(
            sum,
            $ctx.token_balance(&$vault_ta),
            "Σ balances + insurance == vault tokens"
        );
    }};
}

#[test]
fn stress_many_rounds_conserve_and_oi_returns_to_zero() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 300; // 3% — small vs the huge balances, never liquidatable
    let pdas = ctx.init_market(1, 200, 64); // tick 1, 200 ticks, cap 64

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Funded trader pool. Balances are huge so margin/PnL never trips liquidation;
    // this test targets clearing + settlement conservation, not the risk path.
    const DEPOSIT: u64 = 10_000_000;
    // Every trader is sometimes a maker (even traders quote-buy on open, odd
    // traders quote-buy on close) and sometimes a taker, so each gets a persistent
    // MakerQuote inited ONCE here (init is create-once; the ladder is re-posted
    // each round with a strictly-increasing sequence — §1.3 maker liquidity now
    // lives in the quote book, not the slab).
    let mut traders = Vec::with_capacity(N);
    for _ in 0..N {
        let t = ctx.new_funded_signer();
        ctx.init_collateral(&t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, DEPOSIT);
        ctx.deposit(&t, &vault_ta, &ta, DEPOSIT);
        ctx.init_position(&pdas, &t);
        ctx.init_maker_quote(&pdas, &t, None, 0); // expiry 0 = never expire
        traders.push(t);
    }
    let tick_size = ctx.market(&pdas).tick_size;
    let total_deposit = DEPOSIT * N as u64;
    assert_eq!(ctx.token_balance(&vault_ta), total_deposit);

    // Deterministic LCG (no Math.random — reproducible failures).
    let mut seed: u64 = 0x1234_5678_9abc_def0;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };

    let mut price: u64 = 50;
    let pairs = N / 2;
    // Per-maker strictly-increasing quote sequence (UpdateMakerQuoteLevels rejects
    // a non-increasing sequence; it persists across rounds).
    let mut quote_seq = [0u64; N];

    for _round in 0..ROUNDS {
        let p_open = price;
        // Random walk the close price within a band so PnL stays well inside balances.
        let delta = (next() % 21) as i64 - 10; // -10..=+10
        let p_close = (price as i64 + delta).clamp(10, 180) as u64;
        price = p_close;

        // Per-pair quantities (1..=20).
        let qtys: Vec<u64> = (0..pairs).map(|_| 1 + next() % 20).collect();
        let total_q: u128 = qtys.iter().map(|&q| q as u128).sum();

        // --- OPEN auction at p_open: trader[2k] long (maker-buy via the quote
        //     book), trader[2k+1] short (taker-sell), equal qty per pair → clears
        //     fully at p_open. The maker liquidity now comes from the MakerQuote
        //     book (§1.3): submit_order is taker-only.
        //
        //     ALL N quotes are active (inited once in setup), so completeness
        //     requires every quote folded every round. The round's makers
        //     (even traders) post a real one-level bid ladder; everyone else
        //     re-posts an EMPTY ladder so their persistent quote folds to nothing
        //     (otherwise a stale ladder from a prior round would re-fold). Each
        //     write uses a strictly-increasing per-maker sequence. ---
        let open_mid = price_to_tick(tick_size, p_open);
        for ti in 0..N {
            quote_seq[ti] += 1;
            let levels: &[(u16, u64)] = if ti % 2 == 0 {
                &[(0, qtys[ti / 2])] // even traders are the open makers (buy q)
            } else {
                &[] // odd traders are takers this phase → empty ladder
            };
            ctx.update_maker_quote_levels(
                &pdas,
                &traders[ti],
                quote_seq[ti],
                open_mid,
                levels,
                &[],
            );
        }
        let mut open_taker_orders = Vec::new(); // (order_id, trader_index)
        for k in 0..pairs {
            let sid = ctx.submit_order(&pdas, &traders[2 * k + 1], SIDE_SELL, p_open, qtys[k]);
            open_taker_orders.push((sid, 2 * k + 1));
        }
        ctx.process_chunk(&pdas, 0, 64);
        for ti in 0..N {
            ctx.process_maker_quote(&pdas, &traders[ti].pubkey());
        }
        ctx.finalize_clear(&pdas);
        // Opening realizes no PnL (margin lock only) — settle order is irrelevant.
        for &(oid, ti) in &open_taker_orders {
            ctx.settle_fill_with_margin(&pdas, oid, &traders[ti].pubkey());
        }
        for ti in 0..N {
            // Even traders settle a real maker fill; odd traders a zero-fill no-op.
            ctx.settle_maker_quote(&pdas, &traders[ti].pubkey());
        }

        let m = ctx.market(&pdas);
        assert_eq!(m.oi_long, total_q, "all longs open");
        assert_eq!(m.oi_short, total_q, "all shorts open");
        assert_eq!(m.oi_long, m.oi_short, "OI balanced after open");
        assert_conserved!(ctx, traders, vault_ta);

        ctx.start_auction(&pdas);

        // --- CLOSE auction at p_close: long sells (taker-sell), short buys
        //     (maker-buy via the quote book). PnL realized; settle LOSERS first to
        //     fund the pool, so a winner is never paid before its counterparty
        //     settles. Makers settle via settle_maker_quote, takers via
        //     settle_fill_with_margin — the loser-first ordering is preserved
        //     across both settle paths. ---
        let price_up = p_close > p_open;
        let close_mid = price_to_tick(tick_size, p_close);
        // Re-post ladders: odd traders are the close makers (buy q), even traders
        // are takers → empty ladder. All N quotes fold/settle every round.
        for ti in 0..N {
            quote_seq[ti] += 1;
            let levels: &[(u16, u64)] = if ti % 2 == 1 {
                &[(0, qtys[ti / 2])] // odd traders are the close makers (buy q)
            } else {
                &[] // even traders are takers this phase → empty ladder
            };
            ctx.update_maker_quote_levels(
                &pdas,
                &traders[ti],
                quote_seq[ti],
                close_mid,
                levels,
                &[],
            );
        }
        // (trader_index, is_loser, is_maker) — taker-sell longs and maker-buy shorts.
        let mut close_settles = Vec::new();
        let mut close_taker_ids = Vec::new(); // taker (order_id, trader_index)
        for k in 0..pairs {
            let lid = ctx.submit_order(&pdas, &traders[2 * k], SIDE_SELL, p_close, qtys[k]);
            close_taker_ids.push((lid, 2 * k));
            close_settles.push((2 * k, !price_up, false)); // long (taker) loses if price fell
            close_settles.push((2 * k + 1, price_up, true)); // short (maker) loses if price rose
        }
        ctx.process_chunk(&pdas, 0, 64);
        for ti in 0..N {
            ctx.process_maker_quote(&pdas, &traders[ti].pubkey());
        }
        ctx.finalize_clear(&pdas);
        // Settle the FILLED legs losers-first, then winners — each via its own
        // settle path (makers via settle_maker_quote, takers via the margin fill).
        for want_loser in [true, false] {
            for &(ti, is_loser, is_maker) in &close_settles {
                if is_loser != want_loser {
                    continue;
                }
                if is_maker {
                    ctx.settle_maker_quote(&pdas, &traders[ti].pubkey());
                } else {
                    let oid = close_taker_ids
                        .iter()
                        .find(|&&(_, t)| t == ti)
                        .map(|&(o, _)| o)
                        .unwrap();
                    ctx.settle_fill_with_margin(&pdas, oid, &traders[ti].pubkey());
                }
            }
        }
        // The even traders' (taker-this-phase) quotes folded an empty ladder; settle
        // their zero-fill no-op so every active quote is settled before start_auction.
        for k in 0..pairs {
            ctx.settle_maker_quote(&pdas, &traders[2 * k].pubkey());
        }

        let m = ctx.market(&pdas);
        assert_eq!(m.oi_long, 0, "all longs closed");
        assert_eq!(m.oi_short, 0, "all shorts closed");
        assert_conserved!(ctx, traders, vault_ta);

        ctx.start_auction(&pdas);
    }

    // End-to-end: PnL is zero-sum, so every token is still backed and total
    // claims equal the original deposits (no creation/destruction of value).
    let mut sum = ctx.vault().insurance_balance;
    for t in &traders {
        sum += ctx.user_collateral(&t.pubkey()).balance;
    }
    assert_eq!(sum, total_deposit, "fully conserved end to end");
    assert_eq!(
        ctx.token_balance(&vault_ta),
        total_deposit,
        "vault holdings unchanged"
    );
}
