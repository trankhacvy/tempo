use metrics::{describe_counter, describe_gauge};

/// Register the simulation's Prometheus metric descriptions (idempotent).
pub fn register() {
    describe_counter!(
        "sim_orders_submitted_total",
        "Trader order submissions, labelled by result (ok|benign|error|skip)."
    );
    describe_gauge!(
        "sim_inventory",
        "The trader's current signed position size."
    );
    describe_gauge!(
        "sim_free_collateral",
        "The trader's free collateral (unmetered markets report a sentinel)."
    );
    describe_gauge!(
        "sim_orders_per_round",
        "Number of orders the trader built in the most recent Collect window."
    );
}
