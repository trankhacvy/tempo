use metrics::{describe_counter, describe_gauge, describe_histogram};

/// Describe the market maker's metrics so the Prometheus exposition carries
/// HELP/TYPE.
pub fn register() {
    describe_counter!(
        "mm_quotes_posted_total",
        "update_maker_quote_levels sends, labelled by result (ok/benign/error)"
    );
    describe_gauge!("mm_skew_ticks", "Inventory-driven mid shift, in ticks");
    describe_gauge!("mm_inventory", "Current signed position size");
    describe_gauge!("mm_free_collateral", "Free collateral backing the ladder");
    describe_gauge!("mm_ladder_levels", "Total rungs posted (bids + asks)");
    describe_histogram!("mm_post_latency_seconds", "Wall time of one quote post");
}
