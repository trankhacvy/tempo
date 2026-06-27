use metrics::{describe_counter, describe_gauge, describe_histogram};

/// Describe the keeper's metrics so the Prometheus exposition carries HELP/TYPE.
pub fn register() {
    describe_gauge!(
        "keeper_phase",
        "Current auction phase (0=Collect..3=Settling)"
    );
    describe_counter!(
        "keeper_tx_total",
        "Crank instruction sends, labelled by ix and result (ok/benign/error)"
    );
    describe_histogram!(
        "keeper_settle_latency_seconds",
        "Wall time of one settle fan-out"
    );
    describe_gauge!(
        "keeper_slots_since_progress",
        "Slots since the on-chain state last advanced (freeze signal)"
    );
    describe_counter!(
        "keeper_tick_errors_total",
        "Tick failures (snapshot or slot load)"
    );
    describe_gauge!(
        "keeper_funding_age_seconds",
        "Seconds since the market's last funding update"
    );
    describe_counter!(
        "keeper_funding_total",
        "update_funding sends, labelled by result"
    );
}
