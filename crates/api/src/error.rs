use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Typed API failure. Each variant maps to one HTTP status; the body is a small
/// `{error, message}` JSON object so clients can branch on the status and show
/// the message.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("invalid pubkey: {0}")]
    BadPubkey(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("service not ready")]
    NotReady,
    #[error("not indexed: {0}")]
    NotIndexed(&'static str),
    #[error(transparent)]
    Sdk(#[from] tempo_sdk::SdkError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadPubkey(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::NotReady => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::NotIndexed(_) => StatusCode::NOT_IMPLEMENTED,
            ApiError::Sdk(_) | ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(serde_json::json!({
            "error": status.as_u16(),
            "message": self.to_string(),
        }));
        (status, body).into_response()
    }
}
