use metrics::{describe_counter, describe_gauge, describe_histogram};

/// Describe the API's metrics so the Prometheus exposition carries HELP/TYPE.
pub fn register() {
    describe_counter!(
        "api_requests_total",
        "HTTP requests, labelled by path and status class"
    );
    describe_histogram!(
        "api_request_duration_seconds",
        "HTTP request handler latency"
    );
    describe_gauge!(
        "api_ws_connections",
        "Currently connected WebSocket clients"
    );
    describe_counter!(
        "api_watcher_poll_total",
        "Market watcher polls, labelled by result (ok/error)"
    );
    describe_gauge!(
        "api_live_phase",
        "Current auction phase from the latest watcher poll (0=Collect..3=Settling)"
    );
}
