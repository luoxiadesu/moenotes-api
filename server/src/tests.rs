use super::*;
use async_trait::async_trait;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use moenotes_client::{Generation, QueryResponse};
use prost_reflect::DynamicMessage;
use std::{
    sync::{
        RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

const KEY: &str = "synthetic-http-key-not-for-production-123456789";
struct Fake {
    generation: RwLock<Generation>,
    calls: AtomicUsize,
    failure: AtomicBool,
    delay: Duration,
}
impl Fake {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            generation: RwLock::new(Generation::new_v4()),
            calls: AtomicUsize::new(0),
            failure: AtomicBool::new(false),
            delay,
        })
    }
    fn replace(&self) {
        *self.generation.write().unwrap() = Generation::new_v4();
    }
}
#[async_trait]
impl QueryClient for Fake {
    fn generation(&self) -> Generation {
        *self.generation.read().unwrap()
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        cancel: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::select! { _ = tokio::time::sleep(self.delay) => {}, _ = cancel.cancelled() => return Err(ClientError::new(ErrorKind::Cancelled)) }
        if self.failure.load(Ordering::SeqCst) {
            return Err(ClientError::new(ErrorKind::Maintenance));
        }
        let descriptor = moenotes_proto::pool()
            .get_message_by_name(query.method().output)
            .unwrap();
        let message = if query.method().name == "favorite-status" {
            DynamicMessage::deserialize(
                descriptor,
                serde_json::json!({"totalFavorite":"9007199254740993","isSentFavorite":true}),
            )
            .unwrap()
        } else {
            DynamicMessage::new(descriptor)
        };
        Ok(QueryResponse {
            message,
            fetched_at: SystemTime::now(),
            generation,
        })
    }
}
fn q() -> Query {
    Query::from_json("announcement", serde_json::json!({"id":"1"})).unwrap()
}

#[tokio::test]
async fn overflowing_cache_ttl_is_rejected_without_upstream_work() {
    let fake = Fake::new(Duration::ZERO);
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            ttl: Duration::MAX,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let result = tokio::time::timeout(Duration::from_secs(1), cache.query(q()))
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(ClientError {
            kind: ErrorKind::InvalidConfig,
            ..
        })
    ));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}
fn request(path: &str, key: bool, body: serde_json::Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if key {
        request = request.header("authorization", format!("Bearer {KEY}"));
    }
    request.body(Body::from(body.to_string())).unwrap()
}
fn app(fake: Arc<Fake>, enabled: bool) -> Router {
    router(
        fake,
        Zeroizing::new(KEY.into()),
        enabled,
        CacheOptions::default(),
        CancellationToken::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn routes_default_off_auth_health_and_json() {
    let fake = Fake::new(Duration::ZERO);
    let disabled = app(fake.clone(), false);
    let response = disabled
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        disabled
            .oneshot(request(ROUTES[0].0, true, serde_json::json!({"id":"1"})))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    let enabled = app(fake.clone(), true);
    assert_eq!(
        enabled
            .clone()
            .oneshot(request(ROUTES[0].0, false, serde_json::json!({"id":"1"})))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut invalid_key = request(ROUTES[0].0, true, serde_json::json!({"id":"1"}));
    invalid_key
        .headers_mut()
        .append("authorization", "Bearer wrong".parse().unwrap());
    assert_eq!(
        enabled.clone().oneshot(invalid_key).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let response = enabled
        .clone()
        .oneshot(request(
            "/experimental/v1/profiles/favorite-status",
            true,
            serde_json::json!({"playerId":"synthetic"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-moenotes-cache"], "MISS");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().contains_key("x-moenotes-fetched-at"));
    let value: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"totalFavorite":"9007199254740993","isSentFavorite":true})
    );
    let response = enabled
        .clone()
        .oneshot(request(
            "/experimental/v1/profiles/favorite-status",
            true,
            serde_json::json!({"playerId":"synthetic"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.headers()["x-moenotes-cache"], "HIT");
    for body in [
        serde_json::json!({"id":"0"}),
        serde_json::json!({"id":"1","unknown":1}),
        serde_json::json!({"id":"not-an-id"}),
    ] {
        assert_eq!(
            enabled
                .clone()
                .oneshot(request(ROUTES[0].0, true, body))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn all_http_routes_and_openapi() {
    let fake = Fake::new(Duration::ZERO);
    let enabled = app(fake.clone(), true);
    let inputs = [
        serde_json::json!({"id":"1"}),
        serde_json::json!({}),
        serde_json::json!({"arenaSeasonId":"1","rankingStart":"1","rankingEnd":"100"}),
        serde_json::json!({"musicId":"1","arenaSeasonId":"1"}),
        serde_json::json!({"circleId":"1"}),
        serde_json::json!({}),
        serde_json::json!({"options":{}}),
        serde_json::json!({"challengeMusicId":"1"}),
        serde_json::json!({"playerId":"synthetic","eventId":"1"}),
        serde_json::json!({"eventId":"1","ranks":[1,10]}),
        serde_json::json!({"playerProfileId":"1"}),
        serde_json::json!({"gachaId":"1"}),
        serde_json::json!({"musicId":"1"}),
        serde_json::json!({"playerId":"synthetic"}),
        serde_json::json!({"accountIds":["1"]}),
    ];
    for ((path, _), input) in ROUTES.iter().zip(inputs) {
        assert_eq!(
            enabled
                .clone()
                .oneshot(request(path, true, input))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 15);
    let response = enabled
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .header("authorization", format!("Bearer {KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["paths"].as_object().unwrap().len(), 15);
    assert_eq!(
        json["components"]["schemas"]["app.announcement.GetRequest"]["properties"]["id"]["type"],
        "string"
    );
}

#[tokio::test]
async fn coalescing_failures_expiry_and_session_isolation() {
    let fake = Fake::new(Duration::from_millis(15));
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            ttl: Duration::from_millis(30),
            ..CacheOptions::default()
        },
        CancellationToken::new(),
    );
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let cache = cache.clone();
        tasks.push(tokio::spawn(
            async move { cache.query(q()).await.unwrap().1 },
        ));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.query(q()).await.unwrap().1, "HIT");
    tokio::time::sleep(Duration::from_millis(40)).await;
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    fake.replace();
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
    fake.replace();
    fake.failure.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(
            cache.query(q()).await.err().unwrap().kind,
            ErrorKind::Maintenance
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn stale_flight_rejected_and_caller_drop_does_not_poison_cache() {
    let fake = Fake::new(Duration::from_millis(30));
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    fake.replace();
    assert_eq!(
        pending.await.unwrap().err().unwrap().kind,
        ErrorKind::SessionChanged
    );
    assert_eq!(cache.query(q()).await.unwrap().1, "MISS");
    fake.replace();
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) < 3 {
        tokio::task::yield_now().await;
    }
    pending.abort();
    let _ = pending.await;
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_backpressure_shutdown_and_safe_errors() {
    let fake = Fake::new(Duration::from_millis(30));
    let stop = CancellationToken::new();
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            capacity: 1,
            max_inflight: 1,
            ..CacheOptions::default()
        },
        stop.clone(),
    );
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let other = Query::from_json("announcement", serde_json::json!({"id":"2"})).unwrap();
    assert_eq!(
        cache.query(other.clone()).await.err().unwrap().kind,
        ErrorKind::QueueFull
    );
    pending.await.unwrap().unwrap();
    cache.query(other).await.unwrap();
    assert_eq!(cache.query(q()).await.unwrap().1, "MISS");
    stop.cancel();
    assert_eq!(
        cache.query(q()).await.err().unwrap().kind,
        ErrorKind::Cancelled
    );
    fake.failure.store(true, Ordering::SeqCst);
    let response = app(fake, true)
        .oneshot(request(ROUTES[0].0, true, serde_json::json!({"id":"1"})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &response.into_body().collect().await.unwrap().to_bytes()
        )
        .unwrap(),
        serde_json::json!({"error":{"kind":"maintenance"}})
    );
}

#[tokio::test]
async fn bounded_bytes_and_full_request_cache_keys() {
    let fake = Fake::new(Duration::ZERO);
    let no_cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            max_bytes: 1,
            ..CacheOptions::default()
        },
        CancellationToken::new(),
    );
    no_cache.query(q()).await.unwrap();
    no_cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    for ranks in [vec![1, 10], vec![10, 1], vec![1, 10, 1], vec![1, 10]] {
        let query = Query::from_json(
            "event-ranking",
            serde_json::json!({"eventId":"1","ranks":ranks}),
        )
        .unwrap();
        cache.query(query).await.unwrap();
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn malformed_oversized_input_and_weak_key_rejected() {
    let fake = Fake::new(Duration::ZERO);
    assert!(
        router(
            fake.clone(),
            Zeroizing::new("short".into()),
            true,
            CacheOptions::default(),
            CancellationToken::new()
        )
        .is_err()
    );
    let app = app(fake.clone(), true);
    for body in ["{".to_owned(), "x".repeat(65537)] {
        let request = Request::builder()
            .method("POST")
            .uri(ROUTES[0].0)
            .header("authorization", format!("Bearer {KEY}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}
