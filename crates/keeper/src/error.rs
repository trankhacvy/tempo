use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeeperError {
    #[error(transparent)]
    Sdk(#[from] tempo_sdk::SdkError),
    #[error(transparent)]
    Common(#[from] tempo_common::CommonError),
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
