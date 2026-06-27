//! Phase 3 — folding maker quotes into the histogram. Folding is commutative
//! (order-independent), `finalize_clear` is blocked until every active quote is
//! folded (maker completeness), and an expired-but-active quote folds zero yet
//! still unblocks finalization.

use tempo_integration_tests::*;

/// Buckets start after the `[disc(1) ver(1) header(53)]` prefix.
const HIST_BUCKETS_OFFSET: usize = 55;

/// An active maker quote blocks `finalize_clear` until it is folded.
#[test]
fn maker_completeness_blocks_finalize() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 32, 8);
    let maker = ctx.new_funded_signer();
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 20, &[(1, 100)], &[(1, 100)]);
    assert_eq!(ctx.active_maker_quote_count(&pdas), 1);

    // Orders are complete (0 == 0), but the maker quote isn't folded yet.
    ctx.process_chunk(&pdas, 0, 8);
    assert_eq!(ctx.folded_maker_quote_count(&pdas), 0);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "finalize must wait for the maker quote"
    );

    // Fold it → maker completeness satisfied → finalize succeeds.
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    assert_eq!(ctx.folded_maker_quote_count(&pdas), 1);
    assert!(ctx.try_finalize_clear(&pdas).is_ok());
}

/// Folding the same set of quotes in either order yields an identical histogram.
#[test]
fn maker_fold_is_commutative() {
    fn fold_in_order(swap: bool) -> Vec<u8> {
        let mut ctx = TestContext::new();
        let pdas = ctx.init_market(1, 32, 8);
        let m1 = ctx.new_funded_signer();
        let m2 = ctx.new_funded_signer();
        ctx.init_maker_quote(&pdas, &m1, None, 0);
        ctx.init_maker_quote(&pdas, &m2, None, 0);
        ctx.update_maker_quote_levels(&pdas, &m1, 1, 20, &[(1, 100), (2, 50)], &[(1, 80)]);
        ctx.update_maker_quote_levels(&pdas, &m2, 1, 20, &[(2, 40)], &[(1, 60), (3, 30)]);
        ctx.process_chunk(&pdas, 0, 8);
        if swap {
            ctx.process_maker_quote(&pdas, &m2.pubkey());
            ctx.process_maker_quote(&pdas, &m1.pubkey());
        } else {
            ctx.process_maker_quote(&pdas, &m1.pubkey());
            ctx.process_maker_quote(&pdas, &m2.pubkey());
        }
        ctx.account_raw(&pdas.histogram)[HIST_BUCKETS_OFFSET..].to_vec()
    }
    assert_eq!(
        fold_in_order(false),
        fold_in_order(true),
        "histogram buckets are fold-order independent"
    );
}

/// An expired-but-active quote folds zero, but is still counted so it does not
/// wedge finalization.
#[test]
fn expired_quote_folds_zero_but_unblocks() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 32, 8);
    let maker = ctx.new_funded_signer();
    ctx.init_maker_quote(&pdas, &maker, None, 2); // expiry_slots = 2
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 20, &[(1, 100)], &[(1, 100)]);

    // Warp far past the quote's expiry window.
    let deadline = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(deadline + 1000);
    ctx.process_chunk(&pdas, 0, 8);

    let buckets_before = ctx.account_raw(&pdas.histogram)[HIST_BUCKETS_OFFSET..].to_vec();
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    let buckets_after = ctx.account_raw(&pdas.histogram)[HIST_BUCKETS_OFFSET..].to_vec();
    assert_eq!(buckets_before, buckets_after, "expired quote folds zero");

    // Still counted → finalize is unblocked.
    assert_eq!(ctx.folded_maker_quote_count(&pdas), 1);
    assert!(ctx.try_finalize_clear(&pdas).is_ok());
}
