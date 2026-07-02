use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommonError {
    #[error("no RPC URLs provided")]
    NoRpcUrls,
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transaction {0} not confirmed in time")]
    ConfirmTimeout(String),
    #[error("transaction {sig} failed: {err}")]
    TxFailed { sig: String, err: String },
    /// The transaction failed simulation/preflight deterministically (e.g. a program
    /// error like OrderSlabFull) — it will never land, so retrying with a fresh
    /// blockhash is pointless. Kept distinct from `ConfirmTimeout` so the conflict-retry
    /// path does NOT waste 4×30s confirm-waits on a tx that was rejected at send time.
    #[error("transaction simulation failed: {0}")]
    SimulationFailed(String),
    #[error("signer error: {0}")]
    Signer(String),
}
