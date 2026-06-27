//! LiteSVM parameter sweep: measures the CU profile of each
//! hot-path instruction across a grid of (num_ticks, orders_per_auction_cap,
//! chunk_size) and writes a machine-readable `sweep.csv` at the workspace root.
//!
//! Run with:  cargo test -p tempo-integration-tests --test sweep -- --ignored --nocapture
//!
//! LiteSVM is single-threaded, so this captures CU and the account-size ceiling,
//! NOT real write-lock parallelism (that is the devnet stress test's job). The
//! relative profile and scaling with ticks/cap/chunk are the signal.

use std::fmt::Write as _;
use std::path::PathBuf;

use tempo_integration_tests::*;

const TICKS: &[u32] = &[64, 128, 256];
const CAPS: &[u32] = &[16, 64, 128];
const CHUNKS: &[u32] = &[16, 64, 128];

struct Row {
    ticks: u32,
    cap: u32,
    chunk: u32,
    submit_cu: u64,
    chunk_cu: u64,
    finalize_cu: u64,
    settle_cu: u64,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// Fill `n` taker-buy orders at one in-window price (≤8 per trader). `submit_order`
/// is taker-only (§1.3); this measures the slab free-scan / fold CU profile, which
/// is now driven entirely by taker orders (makers live in the quote book, not the
/// slab).
fn fill_orders(ctx: &mut TestContext, pdas: &MarketPdas, n: u32, price: u64) {
    let mut filled = 0u32;
    while filled < n {
        let trader = ctx.new_funded_signer();
        let batch = core::cmp::min(8, n - filled);
        for _ in 0..batch {
            ctx.submit_order(pdas, &trader, SIDE_BUY, price, 5);
            filled += 1;
        }
    }
}

fn measure(ticks: u32, cap: u32, chunk: u32) -> Row {
    let price = u64::from((ticks / 2).max(1));

    // submit_cu: one submit into a near-full slab (exercises the O(n) free scan).
    let submit_cu = {
        let mut ctx = TestContext::new();
        let pdas = ctx.init_market(1, ticks, cap);
        fill_orders(&mut ctx, &pdas, cap.saturating_sub(1), price);
        let trader = ctx.new_funded_signer();
        ctx.try_submit_order(&pdas, &trader, SIDE_BUY, price, 5)
            .expect("submit")
            .compute_units_consumed
    };

    // chunk_cu: fold `chunk` orders in one process_chunk on a full slab.
    let chunk_cu = {
        let mut ctx = TestContext::new();
        let pdas = ctx.init_market(1, ticks, cap);
        fill_orders(&mut ctx, &pdas, cap, price);
        ctx.process_chunk(&pdas, 0, chunk).compute_units_consumed
    };

    // finalize_cu + settle_cu: a crossing book — a maker-buy (quote book) vs a
    // taker-sell. Two takers never cross (§1.3), so the buy side is maker
    // liquidity from the MakerQuote book; settle_cu is the maker-quote settle on a
    // no-margin market (default maint_bps == 0).
    let (finalize_cu, settle_cu) = {
        let mut ctx = TestContext::new();
        let pdas = ctx.init_market(1, ticks, cap.max(2));
        let buyer = ctx.new_funded_signer();
        let seller = ctx.new_funded_signer();
        ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, price, 10);
        ctx.submit_order(&pdas, &seller, SIDE_SELL, price, 10);
        ctx.process_chunk(&pdas, 0, chunk.max(2));
        ctx.process_maker_quote(&pdas, &buyer.pubkey());
        let finalize = ctx.finalize_clear(&pdas).compute_units_consumed;
        let settle = ctx
            .settle_maker_quote_clearing(&pdas, &buyer.pubkey())
            .compute_units_consumed;
        (finalize, settle)
    };

    Row {
        ticks,
        cap,
        chunk,
        submit_cu,
        chunk_cu,
        finalize_cu,
        settle_cu,
    }
}

#[test]
#[ignore = "sweep; run with --ignored to regenerate sweep.csv"]
fn sweep_cu_grid() {
    let mut rows = Vec::new();
    for &ticks in TICKS {
        for &cap in CAPS {
            for &chunk in CHUNKS {
                rows.push(measure(ticks, cap, chunk));
            }
        }
    }

    let mut csv = String::from("ticks,cap,chunk,submit_cu,chunk_cu,finalize_cu,settle_cu\n");
    for r in &rows {
        writeln!(
            csv,
            "{},{},{},{},{},{},{}",
            r.ticks, r.cap, r.chunk, r.submit_cu, r.chunk_cu, r.finalize_cu, r.settle_cu
        )
        .expect("write csv row");
    }
    print!("{csv}");

    let out = workspace_root().join("sweep.csv");
    std::fs::write(&out, csv).expect("write sweep.csv");
    println!("wrote {}", out.display());
}
