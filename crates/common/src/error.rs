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
    #[error("signer error: {0}")]
    Signer(String),
}
