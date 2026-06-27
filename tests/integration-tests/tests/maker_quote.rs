//! Phase 2 — MakerQuote CRUD: init, the O(1) mid hot path, level rewrite, clear,
//! and writer authorization. A mid update touches only the maker's own account.

use tempo_integration_tests::*;

#[test]
fn maker_quote_lifecycle_and_isolation() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 64, 8);
    let maker = ctx.new_funded_signer();

    // init: active, quote_id 0, market counts one active quote.
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    let q = ctx.maker_quote(&pdas, &maker.pubkey());
    assert_eq!(q.status, 1);
    assert_eq!(q.quote_id, 0);
    assert_eq!(q.sequence, 0);
    assert_eq!(ctx.active_maker_quote_count(&pdas), 1);

    // The mid hot path touches ONLY the maker_quote account — the histogram and
    // slab are byte-identical after five updates (zero write contention).
    let hist_before = ctx.account_raw(&pdas.histogram);
    let slab_before = ctx.account_raw(&pdas.order_slab);
    for i in 1..=5u64 {
        ctx.update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, i, (i * 3) as u32);
    }
    let q = ctx.maker_quote(&pdas, &maker.pubkey());
    assert_eq!(q.sequence, 5);
    assert_eq!(q.mid_tick, 15);
    assert_eq!(
        ctx.account_raw(&pdas.histogram),
        hist_before,
        "histogram untouched"
    );
    assert_eq!(
        ctx.account_raw(&pdas.order_slab),
        slab_before,
        "slab untouched"
    );

    // A stale (non-increasing) sequence is rejected.
    assert!(ctx
        .try_update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, 5, 9)
        .is_err());

    // Full ladder rewrite.
    ctx.update_maker_quote_levels(&pdas, &maker, 6, 20, &[(1, 100), (3, 200)], &[(2, 150)]);
    let q = ctx.maker_quote(&pdas, &maker.pubkey());
    assert_eq!(q.num_bids, 2);
    assert_eq!(q.num_asks, 1);
    assert_eq!(q.mid_tick, 20);
    assert_eq!(q.sequence, 6);

    // Clear deactivates and decrements the active count.
    ctx.clear_maker_quote(&pdas, &maker, 7);
    let q = ctx.maker_quote(&pdas, &maker.pubkey());
    assert_eq!(q.status, 0);
    assert_eq!(q.num_bids, 0);
    assert_eq!(ctx.active_maker_quote_count(&pdas), 0);
}

/// known-issues §3: a cleared maker quote can be closed to reclaim its rent, and
/// the freed PDA address re-initialized so the maker can quote again. An active
/// quote cannot be closed, and only the maker (not a stranger) may close it.
#[test]
fn maker_quote_close_reclaims_rent_and_frees_pda() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 64, 8);
    let maker = ctx.new_funded_signer();
    let stranger = ctx.new_funded_signer();

    let quote = ctx.init_maker_quote(&pdas, &maker, None, 0);

    // An active quote cannot be closed — it must be cleared first.
    assert!(
        ctx.try_close_maker_quote(&pdas, &maker).is_err(),
        "active quote must be cleared before close"
    );

    // Clear it (deactivate, drop the active count), then a stranger still can't close it.
    ctx.clear_maker_quote(&pdas, &maker, 1);
    assert!(
        ctx.try_close_maker_quote(&pdas, &stranger).is_err(),
        "only the maker may close"
    );

    // The maker closes the cleared quote: rent returns to the maker and the PDA is gone.
    let maker_before = ctx.svm.get_account(&maker.pubkey()).unwrap().lamports;
    let rent = ctx.svm.get_account(&quote).unwrap().lamports;
    assert!(rent > 0);
    ctx.close_maker_quote(&pdas, &maker);
    let closed = ctx.svm.get_account(&quote);
    assert!(
        closed.as_ref().map(|a| a.lamports).unwrap_or(0) == 0,
        "quote PDA is closed"
    );
    // The maker (not the fee payer) recovers the full rent.
    let maker_after = ctx.svm.get_account(&maker.pubkey()).unwrap().lamports;
    assert_eq!(maker_after, maker_before + rent);

    // The freed deterministic address can be re-initialized — the maker requotes.
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    let q = ctx.maker_quote(&pdas, &maker.pubkey());
    assert_eq!(q.status, 1);
    assert_eq!(ctx.active_maker_quote_count(&pdas), 1);
}

#[test]
fn maker_quote_delegate_authority() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 64, 8);
    let maker = ctx.new_funded_signer();
    let delegate = ctx.new_funded_signer();
    let stranger = ctx.new_funded_signer();

    ctx.init_maker_quote(&pdas, &maker, Some(delegate.pubkey()), 0);

    // The delegate may write the ladder; a stranger may not.
    assert!(ctx
        .try_update_maker_quote_mid(&pdas, &maker.pubkey(), &delegate, 1, 10)
        .is_ok());
    assert!(ctx
        .try_update_maker_quote_mid(&pdas, &maker.pubkey(), &stranger, 2, 11)
        .is_err());
}

#[test]
fn maker_quote_rejects_out_of_window_mid() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 64, 8);
    let maker = ctx.new_funded_signer();
    ctx.init_maker_quote(&pdas, &maker, None, 0);

    // mid_tick must be < num_ticks (64).
    assert!(ctx
        .try_update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, 1, 64)
        .is_err());
    assert!(ctx
        .try_update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, 1, 63)
        .is_ok());
}

/// Audit F3/F4: maker-quote mutators are confined to the `Collect` phase. Once a
/// round advances to `Accumulating` (the quote is folded), `clear_maker_quote` and
/// `update_maker_quote_mid` are rejected — so the completeness counters and the
/// folded ladder cannot move out from under `finalize_clear`/settlement.
#[test]
fn maker_quote_mutation_rejected_after_collect() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 64, 8);
    let maker = ctx.new_funded_signer();

    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 20, &[(1, 100)], &[(2, 100)]);

    // Fold the quote — this closes the collect window and moves to Accumulating.
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    assert_eq!(ctx.folded_maker_quote_count(&pdas), 1);

    // Mid-round mutation must now fail closed (phase gate).
    assert!(
        ctx.try_update_maker_quote_mid(&pdas, &maker.pubkey(), &maker, 2, 30)
            .is_err(),
        "mid update after fold must be rejected"
    );
    assert!(
        ctx.try_clear_maker_quote(&pdas, &maker, 2).is_err(),
        "clear after fold must be rejected"
    );

    // The active/folded counters stay balanced, so finalize is not wedged.
    assert_eq!(ctx.active_maker_quote_count(&pdas), 1);
    assert_eq!(ctx.folded_maker_quote_count(&pdas), 1);
}
