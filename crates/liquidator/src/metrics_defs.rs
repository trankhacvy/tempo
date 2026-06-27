use metrics::{describe_counter, describe_gauge, describe_histogram};

/// Describe the liquidator's metrics so the Prometheus exposition carries HELP/TYPE.
pub fn register() {
    describe_counter!(
        "liquidator_fired_total",
        "Liquidation sends, labelled by kind (isolated/cross) and result (ok/benign/error)"
    );
    describe_gauge!(
        "liquidator_positions_scanned",
        "Isolated positions inspected in the last scan"
    );
    describe_gauge!(
        "liquidator_underwater_count",
        "Positions found below maintenance in the last scan"
    );
    describe_histogram!(
        "liquidator_scan_latency_seconds",
        "Wall time of one full scan"
    );
    describe_counter!(
        "liquidator_scan_errors_total",
        "Scan failures (RPC or decode)"
    );
    describe_gauge!(
        "liquidator_insurance_balance",
        "Vault insurance fund balance (bad-debt backstop)"
    );
}
