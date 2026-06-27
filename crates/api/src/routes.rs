use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Deserialize;
use tempo_sdk::Pubkey;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;

use tempo_sdk::pda;

use crate::dto::{
    AuctionResponse, HistogramResponse, MarketResponse, OrderResponse, PositionResponse,
    QuoteResponse,
};
use crate::error::ApiError;
use crate::history::{FillRow, FundingRow};
use crate::state::AppState;

const MAX_PAGE: u32 = 500;
const DEFAULT_PAGE: u32 = 100;

/// Pagination query (`?limit=&offset=`), clamped so a client cannot request an
/// unbounded slice.
#[derive(Debug, Deserialize)]
pub struct Page {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl Page {
    fn window(&self, total: usize) -> (usize, usize) {
        let limit = self.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE) as usize;
        let offset = self.offset.unwrap_or(0) as usize;
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);
        (start, end)
    }
}

fn parse_pubkey(s: &str) -> Result<Pubkey, ApiError> {
    s.parse::<Pubkey>()
        .map_err(|_| ApiError::BadPubkey(s.to_string()))
}

/// Reject a path that targets a market this instance does not serve (the API is
/// provisioned per market).
fn check_market(state: &AppState, market: &str) -> Result<(), ApiError> {
    let parsed = parse_pubkey(market)?;
    if parsed != state.market {
        return Err(ApiError::NotFound(format!("market {market}")));
    }
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

#[utoipa::path(get, path = "/v1/ready", responses((status = 200), (status = 503)))]
async fn ready(State(state): State<AppState>) -> Result<&'static str, ApiError> {
    state.snapshot()?;
    Ok("ready")
}

#[utoipa::path(
    get, path = "/v1/markets/{market}",
    params(("market" = String, Path, description = "market pubkey")),
    responses((status = 200, body = MarketResponse), (status = 404), (status = 503))
)]
async fn get_market(
    State(state): State<AppState>,
    Path(market): Path<String>,
) -> Result<Json<MarketResponse>, ApiError> {
    check_market(&state, &market)?;
    let live = state.snapshot()?;
    Ok(Json(MarketResponse::from(&live.market)))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/auction",
    params(("market" = String, Path, description = "market pubkey")),
    responses((status = 200, body = AuctionResponse), (status = 404), (status = 503))
)]
async fn get_auction(
    State(state): State<AppState>,
    Path(market): Path<String>,
) -> Result<Json<AuctionResponse>, ApiError> {
    check_market(&state, &market)?;
    let live = state.snapshot()?;
    Ok(Json(AuctionResponse::from_live(&live)))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/histogram",
    params(("market" = String, Path, description = "market pubkey")),
    responses((status = 200, body = HistogramResponse), (status = 404), (status = 503))
)]
async fn get_histogram(
    State(state): State<AppState>,
    Path(market): Path<String>,
) -> Result<Json<HistogramResponse>, ApiError> {
    check_market(&state, &market)?;
    let live = state.snapshot()?;
    Ok(Json(HistogramResponse::from_live(&live)))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/orders",
    params(("market" = String, Path), ("limit" = Option<u32>, Query), ("offset" = Option<u32>, Query)),
    responses((status = 200, body = [OrderResponse]), (status = 404), (status = 503))
)]
async fn get_orders(
    State(state): State<AppState>,
    Path(market): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<OrderResponse>>, ApiError> {
    check_market(&state, &market)?;
    let live = state.snapshot()?;
    let (start, end) = page.window(live.orders.len());
    let out = live.orders[start..end]
        .iter()
        .map(OrderResponse::from)
        .collect();
    Ok(Json(out))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/quotes",
    params(("market" = String, Path), ("limit" = Option<u32>, Query), ("offset" = Option<u32>, Query)),
    responses((status = 200, body = [QuoteResponse]), (status = 404), (status = 503))
)]
async fn get_quotes(
    State(state): State<AppState>,
    Path(market): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<QuoteResponse>>, ApiError> {
    check_market(&state, &market)?;
    let live = state.snapshot()?;
    let (start, end) = page.window(live.quotes.len());
    let out = live.quotes[start..end]
        .iter()
        .map(|(k, q)| QuoteResponse::from_pair(k, q))
        .collect();
    Ok(Json(out))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/positions",
    params(("market" = String, Path), ("limit" = Option<u32>, Query), ("offset" = Option<u32>, Query)),
    responses((status = 200, body = [PositionResponse]), (status = 404), (status = 503))
)]
async fn get_positions(
    State(state): State<AppState>,
    Path(market): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<PositionResponse>>, ApiError> {
    check_market(&state, &market)?;
    let positions = state.positions_snapshot();
    let (start, end) = page.window(positions.len());
    let out = positions[start..end]
        .iter()
        .map(|(k, p)| PositionResponse::from_pair(k, p))
        .collect();
    Ok(Json(out))
}

#[utoipa::path(
    get, path = "/v1/positions/{owner}",
    params(("owner" = String, Path, description = "position owner pubkey")),
    responses((status = 200, body = PositionResponse), (status = 404))
)]
async fn get_owner_position(
    State(state): State<AppState>,
    Path(owner): Path<String>,
) -> Result<Json<PositionResponse>, ApiError> {
    let owner = parse_pubkey(&owner)?;
    let (position_pda, _) = pda::position(&state.market, &owner);
    let found = state
        .positions_snapshot()
        .into_iter()
        .find(|(k, _)| *k == position_pda)
        .map(|(k, p)| PositionResponse::from_pair(&k, &p))
        .ok_or_else(|| ApiError::NotFound(format!("position for {owner}")))?;
    Ok(Json(found))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/fills",
    params(("market" = String, Path), ("limit" = Option<u32>, Query)),
    responses((status = 200, body = [FillRow]), (status = 501, description = "indexer not deployed"))
)]
async fn get_fills(
    State(state): State<AppState>,
    Path(market): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<FillRow>>, ApiError> {
    check_market(&state, &market)?;
    let limit = page.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let rows = state.history.recent_fills(&state.market, limit).await?;
    Ok(Json(rows))
}

#[utoipa::path(
    get, path = "/v1/markets/{market}/funding",
    params(("market" = String, Path), ("limit" = Option<u32>, Query)),
    responses((status = 200, body = [FundingRow]), (status = 501, description = "indexer not deployed"))
)]
async fn get_funding(
    State(state): State<AppState>,
    Path(market): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<FundingRow>>, ApiError> {
    check_market(&state, &market)?;
    let limit = page.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let rows = state.history.funding_history(&state.market, limit).await?;
    Ok(Json(rows))
}

/// The machine-readable OpenAPI document for every REST route.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Tempo API",
        description = "Chain-backed read API for the Tempo DFBA perps DEX"
    ),
    paths(
        ready,
        get_market,
        get_auction,
        get_histogram,
        get_orders,
        get_quotes,
        get_positions,
        get_owner_position,
        get_fills,
        get_funding
    ),
    components(schemas(
        MarketResponse,
        AuctionResponse,
        HistogramResponse,
        crate::dto::CrossResponse,
        OrderResponse,
        QuoteResponse,
        PositionResponse,
        FillRow,
        FundingRow
    ))
)]
pub struct ApiDoc;

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

fn cors_layer(origins: &[String]) -> CorsLayer {
    let base = CorsLayer::new().allow_methods([Method::GET]);
    if origins.iter().any(|o| o == "*") {
        base.allow_origin(Any)
    } else {
        let values: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        base.allow_origin(values)
    }
}

/// The core router (no rate limit — that layer needs peer-IP connect info and is
/// added in `main`; tests build this directly). Carries CORS, compression,
/// request timeout, body limit, and tracing.
pub fn router(state: AppState, metrics: PrometheusHandle, cors_origins: &[String]) -> Router {
    let api = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/ready", get(ready))
        .route("/v1/markets/:market", get(get_market))
        .route("/v1/markets/:market/auction", get(get_auction))
        .route("/v1/markets/:market/histogram", get(get_histogram))
        .route("/v1/markets/:market/orders", get(get_orders))
        .route("/v1/markets/:market/quotes", get(get_quotes))
        .route("/v1/markets/:market/positions", get(get_positions))
        .route("/v1/markets/:market/fills", get(get_fills))
        .route("/v1/markets/:market/funding", get(get_funding))
        .route("/v1/positions/:owner", get(get_owner_position))
        .route("/v1/ws/:market", get(crate::ws::ws_handler))
        .with_state(state);

    let docs = Router::new().route("/v1/openapi.json", get(openapi_json));
    let metrics_route = Router::new().route(
        "/metrics",
        get(move || {
            let h = metrics.clone();
            async move { h.render() }
        }),
    );

    api.merge(docs)
        .merge(metrics_route)
        .layer(cors_layer(cors_origins))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(15),
        ))
        .layer(RequestBodyLimitLayer::new(16 * 1024))
        .layer(TraceLayer::new_for_http())
}
