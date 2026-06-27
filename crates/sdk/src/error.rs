use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[cfg(feature = "client")]
    #[error(transparent)]
    Common(#[from] tempo_common::CommonError),
    #[error("account decode error: {0}")]
    Decode(String),
    #[error("account not found: {0}")]
    AccountNotFound(String),
    #[error("price out of the histogram window")]
    PriceOutOfWindow,
    #[error("insufficient free collateral: need {need}, have {have}")]
    InsufficientCollateral { need: u64, have: u64 },
}
