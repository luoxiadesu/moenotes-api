//! Experimental HTTP gateway. Raw account-dependent data is opt-in, not public-safe.
pub mod cache;
pub mod config;
mod openapi;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cache::{CacheOptions, QueryCache};
use moenotes_client::{CancellationToken, ClientError, ErrorKind, Query, QueryClient};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub const ROUTES: &[(&str, &str)] = &[
    ("/experimental/v1/announcements/detail", "announcement"),
    ("/experimental/v1/announcements/list", "announcements"),
    ("/experimental/v1/arena/ranking", "arena-ranking"),
    ("/experimental/v1/arena/deck-trend", "deck-trend"),
    ("/experimental/v1/circles/detail", "circle"),
    (
        "/experimental/v1/circles/recommendations",
        "circle-recommendations",
    ),
    ("/experimental/v1/circles/search", "circle-search"),
    (
        "/experimental/v1/events/challenge-ranking",
        "challenge-ranking",
    ),
    ("/experimental/v1/events/deck", "event-deck"),
    ("/experimental/v1/events/ranking", "event-ranking"),
    ("/experimental/v1/profiles/find", "profile"),
    ("/experimental/v1/gacha/probability", "probability"),
    ("/experimental/v1/music/ranking", "music-ranking"),
    (
        "/experimental/v1/profiles/favorite-status",
        "favorite-status",
    ),
    ("/experimental/v1/profiles/batch", "profiles"),
];

#[derive(Clone)]
struct ApiState {
    cache: Arc<QueryCache>,
    key_hash: [u8; 32],
    admission: Arc<tokio::sync::Semaphore>,
}

pub fn router(
    client: Arc<dyn QueryClient>,
    api_key: Zeroizing<String>,
    enable_raw: bool,
    options: CacheOptions,
    stop: CancellationToken,
) -> Result<Router, ClientError> {
    options.validate()?;
    if api_key.len() < 32
        || api_key.len() > 4096
        || !api_key.is_ascii()
        || api_key
            .bytes()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return Err(ClientError::new(ErrorKind::InvalidConfig));
    }
    let state = ApiState {
        cache: QueryCache::new(client, options, stop),
        key_hash: Sha256::digest(api_key.as_bytes()).into(),
        admission: Arc::new(tokio::sync::Semaphore::new(64)),
    };
    let mut protected =
        Router::new().route("/openapi.json", get(|| async { Json(openapi::document()) }));
    if enable_raw {
        for &(path, name) in ROUTES {
            protected = protected.route(
                path,
                post(
                    move |State(state): State<ApiState>,
                          body: Result<
                        Json<serde_json::Value>,
                        axum::extract::rejection::JsonRejection,
                    >| async move {
                        let Json(value) = body
                            .map_err(|_| HttpError(ClientError::new(ErrorKind::InvalidRequest)))?;
                        let query = Query::from_json(name, value).map_err(HttpError)?;
                        let (response, status) =
                            state.cache.query(query).await.map_err(HttpError)?;
                        let mut headers = HeaderMap::new();
                        headers.insert("cache-control", "no-store".parse().unwrap());
                        headers.insert("x-moenotes-cache", status.parse().unwrap());
                        headers.insert(
                            "x-moenotes-fetched-at",
                            unix_millis(response.fetched_at)
                                .to_string()
                                .parse()
                                .unwrap(),
                        );
                        Ok::<_, HttpError>((headers, Json(response.json.clone())))
                    },
                ),
            );
        }
    }
    protected = protected
        .layer(DefaultBodyLimit::max(64 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Ok(Router::new()
        .route(
            "/healthz",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .merge(protected)
        .with_state(state))
}

async fn authenticate(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let values = request
        .headers()
        .get_all("authorization")
        .iter()
        .collect::<Vec<_>>();
    let provided = if values.len() == 1 {
        values[0]
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
    } else {
        None
    };
    let valid = provided.filter(|v| v.len() <= 4096).is_some_and(|v| {
        let hash: [u8; 32] = Sha256::digest(v.as_bytes()).into();
        bool::from(hash.ct_eq(&state.key_hash))
    });
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            [
                ("cache-control", "no-store"),
                ("www-authenticate", "Bearer"),
            ],
            Json(serde_json::json!({"error":{"kind":"unauthorized"}})),
        )
            .into_response();
    }
    let Ok(_permit) = state.admission.try_acquire() else {
        return HttpError(ClientError::new(ErrorKind::QueueFull)).into_response();
    };
    next.run(request).await
}

struct HttpError(ClientError);
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.kind {
            ErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
            ErrorKind::QueueFull => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorKind::Maintenance
            | ErrorKind::Authentication
            | ErrorKind::AuthenticationRequired
            | ErrorKind::Version
            | ErrorKind::DeviceConflict
            | ErrorKind::SessionChanged
            | ErrorKind::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        };
        (
            status,
            [("cache-control", "no-store")],
            Json(serde_json::json!({"error":{"kind":self.0.kind}})),
        )
            .into_response()
    }
}

fn unix_millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests;
