//! Phase 4 — settle a maker quote's fills end-to-end against a taker order, with
//! the full margin money path, and verify conservation (vault tokens == Σ
//! balances + insurance).

use tempo_integration_tests::*;

#[test]
fn maker_quote_settles_against_taker_and_conserves() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // margin-enabled money path
    let mint = ctx.create_mint();
    ctx.market_collateral_mint = Some(mint);
    let pdas = ctx.init_market(1, 32, 8);

    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let maker = ctx.new_funded_signer();
    let taker = ctx.new_funded_signer();
    for t in [&maker, &taker] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 10_000);
        ctx.deposit(t, &vault_ta, &ta, 10_000);
        ctx.init_position(&pdas, t);
    }
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Maker quotes a bid (buy 10 at tick 20 → price 21); taker sells 10 at price
    // 21 → they cross in the bid auction.
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 20, &[(0, 10)], &[]);
    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, 21, 10);

    // Crank: fold both sources, finalize, settle both.
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());
    ctx.settle_maker_quote(&pdas, &maker.pubkey());

    let (mpos, _) = ctx.position_pda(&pdas, &maker.pubkey());
    let (tpos, _) = ctx.position_pda(&pdas, &taker.pubkey());
    let mp = ctx.position(&mpos);
    let tp = ctx.position(&tpos);
    assert_eq!(mp.size, 10, "maker is long 10 (bought via its quote)");
    assert_eq!(mp.entry_price, 21);
    assert_eq!(tp.size, -10, "taker is short 10");
    assert_eq!(tp.entry_price, 21);

    // Conservation: nothing was minted or lost.
    let mb = ctx.user_collateral(&maker.pubkey()).balance;
    let tb = ctx.user_collateral(&taker.pubkey()).balance;
    let cb = ctx.user_collateral(&cranker.pubkey()).balance;
    let ins = ctx.vault().insurance_balance;
    assert_eq!(
        mb + tb + cb + ins,
        ctx.token_balance(&vault_ta),
        "claims + insurance == vault tokens"
    );
    assert_eq!(mb + tb + cb + ins, 20_000);
}

/// §1.6 regression: TWO makers resting at the EXACT same rationed marginal tick
/// must split the allocated volume so their fills sum to exactly `vol_alloc` — not
/// each take a full first-slice as if it were alone (which broke conservation: the
/// scarce taker side filled `V` while the maker side filled `< V`, and the gap was
/// silently absorbed by the insurance pool). The fold-time `cum_before` snapshots
/// give each maker its contiguous prefix, so OI conserves: Σ maker longs == taker
/// short == V.
#[test]
fn two_makers_share_marginal_tick_and_conserve_oi() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // margin-enabled money path
    let mint = ctx.create_mint();
    ctx.market_collateral_mint = Some(mint);
    let pdas = ctx.init_market(1, 32, 8);

    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let maker1 = ctx.new_funded_signer();
    let maker2 = ctx.new_funded_signer();
    let taker = ctx.new_funded_signer();
    for t in [&maker1, &maker2, &taker] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 10_000);
        ctx.deposit(t, &vault_ta, &ta, 10_000);
        ctx.init_position(&pdas, t);
    }
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Both makers buy 10 at tick 20 (price 21) — demand[20] = 20. The taker sells
    // only 7 at price 21, so the bid auction crosses at tick 20 with V = 7 and the
    // DEMAND (maker) side is rationed: vol_alloc = 7 over a 20-lot bucket.
    for m in [&maker1, &maker2] {
        ctx.init_maker_quote(&pdas, m, None, 0);
        ctx.update_maker_quote_levels(&pdas, m, 1, 20, &[(0, 10)], &[]);
    }
    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, 21, 7);

    // Crank: fold both quotes (maker1 first → cum_before 0; maker2 → cum_before 10),
    // the taker order, finalize, then settle everyone.
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker1.pubkey());
    ctx.process_maker_quote(&pdas, &maker2.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());
    ctx.settle_maker_quote(&pdas, &maker1.pubkey());
    ctx.settle_maker_quote(&pdas, &maker2.pubkey());

    let (m1pos, _) = ctx.position_pda(&pdas, &maker1.pubkey());
    let (m2pos, _) = ctx.position_pda(&pdas, &maker2.pubkey());
    let (tpos, _) = ctx.position_pda(&pdas, &taker.pubkey());
    let m1 = ctx.position(&m1pos);
    let m2 = ctx.position(&m2pos);
    let tp = ctx.position(&tpos);

    // Telescoping floor: floor(10·7/20)=3 for the first folded maker, 7-3=4 for the
    // second. The exact split is fold-order-determined; the SUM is the invariant.
    assert_eq!(m1.size, 3, "first-folded maker gets the floor slice");
    assert_eq!(
        m2.size, 4,
        "second-folded maker gets the telescoped remainder"
    );
    assert_eq!(
        m1.size + m2.size,
        7,
        "maker longs sum to exactly the allocated volume (no over/under-fill)"
    );
    assert_eq!(
        tp.size, -7,
        "taker short matches the maker longs — OI conserved"
    );

    // Money path conserves: nothing minted or lost across the shared marginal tick.
    let b1 = ctx.user_collateral(&maker1.pubkey()).balance;
    let b2 = ctx.user_collateral(&maker2.pubkey()).balance;
    let tb = ctx.user_collateral(&taker.pubkey()).balance;
    let cb = ctx.user_collateral(&cranker.pubkey()).balance;
    let ins = ctx.vault().insurance_balance;
    assert_eq!(
        b1 + b2 + tb + cb + ins,
        ctx.token_balance(&vault_ta),
        "claims + insurance == vault tokens"
    );
    assert_eq!(b1 + b2 + tb + cb + ins, 30_000);
}
