use thiserror::Error;

#[derive(Debug, Error)]
pub enum SimError {
    #[error(transparent)]
    Common(tempo_common::CommonError),

    #[error(transparent)]
    Sdk(#[from] tempo_sdk::SdkError),

    #[error("config: {0}")]
    Config(String),

    #[error("provisioning: {0}")]
    Provision(String),

    #[error("rpc: {0}")]
    Rpc(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}
