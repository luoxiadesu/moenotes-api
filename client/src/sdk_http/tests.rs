use super::*;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use rsa::{RsaPrivateKey, pkcs8::EncodePublicKey};
use serde_json::{Value, json};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

fn parameters() -> CommonParameters {
    CommonParameters::new(
        COMMON_FIELDS
            .iter()
            .map(|k| (k.to_string(), Some(format!("synthetic-{k}"))))
            .collect(),
    )
    .unwrap()
}
fn client() -> SdkHttpClient {
    SdkHttpClient::new(
        SdkHttpConfig {
            base_url: "https://sdk.invalid".into(),
            allowed_base_urls: vec!["https://sdk.invalid".into()],
        },
        parameters(),
        AppKey::new("synthetic-app-key".into()).unwrap(),
        SdkHeaders::default(),
        SdkHttpOptions {
            minimum_interval: Duration::ZERO,
            ..Default::default()
        },
    )
    .unwrap()
}
fn private_key() -> &'static RsaPrivateKey {
    static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
    KEY.get_or_init(|| RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).unwrap())
}
fn challenge(binding: Generation) -> RsaChallenge {
    RsaChallenge {
        binding,
        hash: Zeroizing::new("synthetic-hash".into()),
        pem: Zeroizing::new(
            private_key()
                .to_public_key()
                .to_public_key_pem(Default::default())
                .unwrap(),
        ),
    }
}
fn password() -> PasswordLogin {
    PasswordLogin {
        user_id: "synthetic+user@example.invalid".into(),
        password: "synthetic-password".into(),
        account_type: PasswordAccountType::Legacy,
        ticket: None,
        third_payment_voucher: None,
    }
}
fn cached() -> CacheLogin {
    CacheLogin {
        access_key: "synthetic-cached-key".into(),
        ticket: None,
        third_payment_voucher: None,
        current_user: None,
    }
}
fn success<T>(value: SdkReply<T>) -> T {
    match value {
        SdkReply::Success(v) => v,
        _ => panic!("expected success"),
    }
}

#[test]
fn okhttp_oracle_encoding_and_signing() {
    let vectors: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/sdk-form.json")).unwrap();
    for case in vectors["cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let result = if case["mode"] == "raw" {
            form::raw(input)
        } else {
            form::encoded(input)
        };
        assert_eq!(result.as_str(), case["encoded"].as_str().unwrap());
        assert_eq!(
            form::decoded(&result).unwrap().as_str(),
            case["decoded"].as_str().unwrap()
        );
    }
    let body = form::sign_form(
        vec![
            ("z".into(), form::encoded("old")),
            ("a".into(), form::encoded("a+b%2Bc")),
            ("z".into(), form::encoded("new%20value")),
            ("ITEM_NAME".into(), form::encoded("ignored")),
        ],
        "synthetic-app-key",
    )
    .unwrap();
    assert!(body.ends_with(vectors["signature"].as_str().unwrap()));
    assert!(body.starts_with("z=old&a=a+b%2Bc&z=new%20value&ITEM_NAME=ignored&sign="));
    assert!(form::decoded("%ff").is_err());
}

#[test]
fn form_common_fields_timestamp_and_nulls() {
    let mut client = client();
    let mut values = COMMON_FIELDS
        .iter()
        .map(|k| (k.to_string(), Some("a+b%2B".into())))
        .collect::<BTreeMap<_, _>>();
    values.insert("adid".into(), None);
    client.parameters = CommonParameters::new(values).unwrap();
    let body = client
        .form(
            vec![("user_id".into(), Zeroizing::new("a+b%2B".into()))],
            false,
            None,
            1234567890123,
        )
        .unwrap();
    let fields = url::form_urlencoded::parse(body.as_bytes()).collect::<BTreeMap<_, _>>();
    assert_eq!(fields["user_id"], "a+b%2B");
    assert_eq!(fields["model"], "a b+");
    assert_eq!(fields["timestamp"], "1234567890123");
    assert_eq!(fields["web_code"], "6");
    assert!(!fields.contains_key("adid"));
    assert!(!fields.contains_key("uid"));
    let current = CurrentSdkUser {
        uid: "synthetic-uid".into(),
        mid: "synthetic-mid".into(),
    };
    let body = client
        .form(
            vec![("access_key".into(), form::encoded("x+y"))],
            true,
            Some(&current),
            1,
        )
        .unwrap();
    assert!(body.contains("uid=synthetic-uid"));
    assert!(body.contains("mid=synthetic-mid"));
}

#[test]
fn rsa_encrypts_hash_then_password_with_pkcs1_v15() {
    let challenge = challenge(Generation::new_v4());
    let encrypted = encrypt_password(&challenge, "synthetic-password").unwrap();
    let plain = private_key()
        .decrypt(
            Pkcs1v15Encrypt,
            &STANDARD.decode(encrypted.as_bytes()).unwrap(),
        )
        .unwrap();
    assert_eq!(plain, b"synthetic-hashsynthetic-password");
    assert_ne!(
        encrypted,
        encrypt_password(&challenge, "synthetic-password").unwrap()
    );
    assert!(encrypt_password(&challenge, &"x".repeat(128)).is_err());
}

#[test]
fn typed_responses_are_strict_redacted_and_keep_cached_key() {
    let body = br#"{"code":0,"data":{"uid":"synthetic-uid","access_key":"synthetic-response-key","id_token":"synthetic-id-token","expires":123,"refresh_token":"synthetic-refresh"}}"#;
    let user = success(response::login(body, None).unwrap());
    assert_eq!(user.access_key(), "synthetic-response-key");
    assert_eq!(user.raw_expires(), Some(123));
    assert!(user.has_refresh_token());
    assert!(!format!("{user:?}").contains("synthetic"));
    let cached = success(response::login(body, Some("synthetic-old-key")).unwrap());
    assert_eq!(cached.access_key(), "synthetic-old-key");
    for bad in [
        r#"{}"#,
        r#"{"code":0}"#,
        r#"{"code":0,"data":null}"#,
        r#"{"code":0,"data":{}}"#,
        r#"{"code":0,"data":{"uid":"x","access_key":""}}"#,
        r#"{"code":"0","data":{}}"#,
        r#"{"code":0,"code":200007,"data":{}}"#,
    ] {
        assert_eq!(
            response::login(bad.as_bytes(), None).unwrap_err().kind,
            SdkErrorKind::Protocol
        );
    }
    for code in [-101, 200007, 800011, 987654] {
        let body = json!({"code":code,"message":"synthetic-secret-message", "data":{"redirect_url":"synthetic-secret-redirect","access_key":"synthetic-secret-key"}}).to_string();
        let SdkReply::Rejected(rejection) = response::login(body.as_bytes(), None).unwrap() else {
            panic!()
        };
        assert_eq!(rejection.code(), code);
        assert_eq!(rejection.requires_challenge(), code == 200007);
        assert_eq!(rejection.requires_account_restore(), code == 800011);
        assert!(!format!("{rejection:?}").contains("synthetic"));
        assert!(
            rejection
                .sensitive_data_json()
                .contains("synthetic-secret-key")
        );
    }
}

#[tokio::test]
async fn config_input_validation_and_instance_binding() {
    for base in [
        "http://sdk.invalid",
        "https://user@sdk.invalid",
        "https://sdk.invalid/bad",
        "https://sdk.invalid?x=1",
        "https://sdk.invalid#x",
    ] {
        assert!(normalize_base(base).is_err());
    }
    assert_eq!(
        normalize_base("https://SDK.invalid:443/passport/").unwrap(),
        "https://sdk.invalid/passport"
    );
    let config = SdkHttpConfig {
        base_url: "https://sdk.invalid".into(),
        allowed_base_urls: vec!["https://other.invalid".into()],
    };
    assert!(
        SdkHttpClient::new(
            config,
            parameters(),
            AppKey::new("x".into()).unwrap(),
            SdkHeaders::default(),
            Default::default()
        )
        .is_err()
    );
    assert!(CommonParameters::new(BTreeMap::new()).is_err());
    let client = client();
    let err = client
        .password_login(
            &challenge(Generation::new_v4()),
            &password(),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, SdkErrorKind::InvalidInput);
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        client.fetch_rsa(None, token).await.unwrap_err().kind,
        SdkErrorKind::Cancelled
    );
    for text in [
        format!("{:?}", password()),
        format!("{:?}", cached()),
        format!("{:?}", client.key),
        format!("{:?}", client.parameters),
    ] {
        assert!(!text.contains("synthetic"));
    }
}

type Calls = Arc<std::sync::Mutex<Vec<(String, HeaderMap, String)>>>;
#[derive(Clone)]
struct MockState {
    calls: Calls,
    mode: u8,
    started: Arc<AtomicUsize>,
}
async fn handler(
    State(state): State<MockState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    state.calls.lock().unwrap().push((
        uri.path().into(),
        headers,
        String::from_utf8(body.to_vec()).unwrap(),
    ));
    state.started.fetch_add(1, Ordering::SeqCst);
    match state.mode {
        1 => {
            return (
                StatusCode::FOUND,
                [("location", "/must-not-follow")],
                "synthetic-secret",
            )
                .into_response();
        }
        2 => return "x".repeat(MAX_RESPONSE + 1).into_response(),
        3 => tokio::time::sleep(Duration::from_millis(150)).await,
        4 => return (StatusCode::SERVICE_UNAVAILABLE, "synthetic-secret").into_response(),
        5 => {
            return axum::Json(json!({"code":200007,"data":{"redirect_url":"synthetic-private"}}))
                .into_response();
        }
        6 => {
            let stream = futures_util::stream::iter([
                Ok::<_, std::convert::Infallible>(Bytes::from(vec![b'x'; MAX_RESPONSE / 2 + 1])),
                Ok(Bytes::from(vec![b'y'; MAX_RESPONSE / 2 + 1])),
            ]);
            return axum::response::Response::new(axum::body::Body::from_stream(stream));
        }
        7 => return StatusCode::NO_CONTENT.into_response(),
        8 => return StatusCode::RESET_CONTENT.into_response(),
        _ => {}
    }
    let data = if uri.path().ends_with(RSA_PATH) {
        json!({"rsa_key": challenge(Generation::new_v4()).pem.as_str(),"hash":"synthetic-hash"})
    } else {
        json!({"uid":"synthetic-uid","access_key":"synthetic-returned-key","id_token":"synthetic-id"})
    };
    axum::Json(json!({"code":0,"data":data})).into_response()
}
async fn mock(
    mode: u8,
) -> (
    SdkHttpClient,
    MockState,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let state = MockState {
        calls: Arc::default(),
        mode,
        started: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let router = Router::new().fallback(handler).with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let mut client = client();
    // Test-only mutation. Production constructor has no plaintext or redirect escape hatch.
    client.config.base_url = format!("http://{addr}/passport");
    client.http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    (client, state, stop, task)
}

#[tokio::test]
async fn three_http_methods_use_expected_headers_paths_bodies() {
    let (mut client, state, stop, task) = mock(0).await;
    client.headers.trace_id = Some("synthetic-trace".into());
    client.headers.one_sdk_version = Some("synthetic-version".into());
    let rsa = success(
        client
            .fetch_rsa(None, CancellationToken::new())
            .await
            .unwrap(),
    );
    let mut login = password();
    login.account_type = PasswordAccountType::Email { country_id: 42 };
    login.ticket = Some("synthetic+ticket%".into());
    let result = success(
        client
            .password_login(&rsa, &login, None, CancellationToken::new())
            .await
            .unwrap(),
    );
    assert_eq!(result.access_key(), "synthetic-returned-key");
    let result = success(
        client
            .cache_login(&cached(), CancellationToken::new())
            .await
            .unwrap(),
    );
    assert_eq!(result.access_key(), "synthetic-cached-key");
    {
        let calls = state.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for ((path, headers, body), suffix) in
            calls.iter().zip([RSA_PATH, PASSWORD_PATH, CACHE_PATH])
        {
            assert_eq!(path, &format!("/passport{suffix}"));
            assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
            assert_eq!(headers["user-agent"], "Mozilla/5.0 BSGameSDK");
            assert_eq!(headers["api-version"], "1");
            assert_eq!(headers["one-sdk-ver"], "synthetic-version");
            assert_eq!(headers["x-game-trace-id"], "synthetic-trace");
            assert!(headers.get("authorization").is_none());
            assert!(headers.get("x-player-credential").is_none());
            let fields = url::form_urlencoded::parse(body.as_bytes()).collect::<BTreeMap<_, _>>();
            assert_eq!(fields["web_code"], "6");
            assert!(fields["timestamp"].parse::<u64>().unwrap() > 1_000_000_000_000);
            let mut hasher = md5::Md5::new();
            use md5::Digest;
            for (name, value) in &fields {
                if name != "sign" {
                    hasher.update(value.as_bytes());
                }
            }
            hasher.update("synthetic-app-key");
            assert_eq!(fields["sign"], format!("{:x}", hasher.finalize()));
            if suffix == PASSWORD_PATH {
                assert_eq!(fields["user_id"], login.user_id);
                assert_eq!(fields["ticket"], "synthetic+ticket%");
                assert_eq!(fields["user_id_type"], "0");
                assert_eq!(fields["country_id"], "42");
                let decrypted = private_key()
                    .decrypt(
                        Pkcs1v15Encrypt,
                        &STANDARD.decode(fields["pwd"].as_bytes()).unwrap(),
                    )
                    .unwrap();
                assert_eq!(decrypted, b"synthetic-hashsynthetic-password");
            }
        }
        assert_ne!(
            calls[0].1["x-game-request-id"],
            calls[1].1["x-game-request-id"]
        );
    }
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn errors_redirects_and_oversize_are_single_shot() {
    for (mode, kind) in [
        (1, SdkErrorKind::HttpStatus),
        (2, SdkErrorKind::Protocol),
        (4, SdkErrorKind::HttpStatus),
        (6, SdkErrorKind::Protocol),
        (7, SdkErrorKind::HttpStatus),
        (8, SdkErrorKind::HttpStatus),
    ] {
        let (client, state, stop, task) = mock(mode).await;
        let err = client
            .cache_login(&cached(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.kind, kind);
        assert!(!format!("{err:?}").contains("synthetic"));
        assert_eq!(state.calls.lock().unwrap().len(), 1);
        stop.cancel();
        task.await.unwrap();
    }
    let (client, state, stop, task) = mock(5).await;
    let SdkReply::Rejected(r) = client
        .cache_login(&cached(), CancellationToken::new())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(r.requires_challenge());
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn cancellation_deadline_and_busy_limit() {
    let (mut client, state, stop, task) = mock(3).await;
    client.options.timeout = Duration::from_millis(30);
    assert_eq!(
        client
            .cache_login(&cached(), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        SdkErrorKind::Timeout
    );
    client.options.timeout = Duration::from_secs(2);
    let client = Arc::new(client);
    let token = CancellationToken::new();
    let cancel = token.clone();
    let c = client.clone();
    let pending = tokio::spawn(async move { c.cache_login(&cached(), cancel).await });
    while state.started.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        client
            .cache_login(&cached(), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        SdkErrorKind::Busy
    );
    token.cancel();
    assert_eq!(
        pending.await.unwrap().unwrap_err().kind,
        SdkErrorKind::Cancelled
    );
    assert_eq!(state.calls.lock().unwrap().len(), 2);
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn root_base_account_types_and_encoded_cache_fields() {
    let (mut client, state, stop, task) = mock(0).await;
    client.config.base_url = client.config.base_url.trim_end_matches("/passport").into();
    let rsa = challenge(client.binding);
    for account_type in [
        PasswordAccountType::Legacy,
        PasswordAccountType::Phone { country_id: 81 },
    ] {
        let mut login = password();
        login.account_type = account_type;
        login.ticket = Some(String::new());
        login.third_payment_voucher = Some("synthetic+voucher%".into());
        client
            .password_login(&rsa, &login, None, CancellationToken::new())
            .await
            .unwrap();
    }
    let mut login = cached();
    login.access_key = "synthetic+key%2B".into();
    login.ticket = Some("synthetic+ticket%2B".into());
    login.current_user = Some(CurrentSdkUser {
        uid: "synthetic-user".into(),
        mid: "synthetic-mid".into(),
    });
    let result = success(
        client
            .cache_login(&login, CancellationToken::new())
            .await
            .unwrap(),
    );
    assert_eq!(result.access_key(), "synthetic+key%2B");
    {
        let calls = state.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for (i, (path, _, body)) in calls.iter().enumerate() {
            let fields = url::form_urlencoded::parse(body.as_bytes()).collect::<BTreeMap<_, _>>();
            if i < 2 {
                assert_eq!(path, PASSWORD_PATH);
                assert!(!fields.contains_key("ticket"));
                assert_eq!(fields["third_payment_voucher"], "synthetic+voucher%");
                if i == 0 {
                    assert!(!fields.contains_key("user_id_type"));
                    assert!(!fields.contains_key("country_id"));
                } else {
                    assert_eq!(fields["user_id_type"], "1");
                    assert_eq!(fields["country_id"], "81");
                }
            } else {
                assert_eq!(path, CACHE_PATH);
                assert_eq!(fields["access_key"], "synthetic key+");
                assert_eq!(fields["ticket"], "synthetic ticket+");
                assert_eq!(fields["uid"], "synthetic-user");
                assert_eq!(fields["mid"], "synthetic-mid");
            }
        }
    }
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn rate_wait_is_cancellable_and_included_in_deadline() {
    let (mut client, state, stop, task) = mock(0).await;
    client.options.minimum_interval = Duration::from_secs(1);
    client
        .cache_login(&cached(), CancellationToken::new())
        .await
        .unwrap();
    assert!(*client.next.lock().await > Instant::now());
    // Avoid wall-clock scheduling assumptions in the timeout/cancellation checks.
    *client.next.lock().await = Instant::now() + Duration::from_secs(30);
    client.options.timeout = Duration::from_millis(20);
    assert_eq!(
        client
            .cache_login(&cached(), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        SdkErrorKind::Timeout
    );
    client.options.timeout = Duration::from_secs(1);
    let token = CancellationToken::new();
    let cancel = token.clone();
    let login = cached();
    let (result, ()) = tokio::join!(client.cache_login(&login, token), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
    });
    assert_eq!(result.unwrap_err().kind, SdkErrorKind::Cancelled);
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    *client.next.lock().await = Instant::now();
    client
        .cache_login(&cached(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(state.calls.lock().unwrap().len(), 2);
    stop.cancel();
    task.await.unwrap();
}

#[test]
fn defensive_input_and_response_limits() {
    assert!(AppKey::new(String::new()).is_err());
    assert!(AppKey::new("x".repeat(4097)).is_err());
    let mut values: BTreeMap<_, _> = COMMON_FIELDS
        .iter()
        .map(|k| (k.to_string(), None))
        .collect();
    values.insert("unknown".into(), None);
    assert!(CommonParameters::new(values.clone()).is_err());
    values.remove("unknown");
    values.insert("udid".into(), Some("x".repeat(4097)));
    assert!(CommonParameters::new(values).is_err());
    assert!(form::sign_form(vec![("x".into(), Zeroizing::new("x".repeat(65536)))], "key").is_err());
    let body = json!({"code":0,"data":{"uid":"x","access_key":"x".repeat(16385)}}).to_string();
    assert_eq!(
        response::login(body.as_bytes(), None).unwrap_err().kind,
        SdkErrorKind::Protocol
    );
    let body = json!({"code":0,"data":{"rsa_key":"not-a-key","hash":"x"}}).to_string();
    assert_eq!(
        response::rsa(body.as_bytes(), Generation::new_v4())
            .unwrap_err()
            .kind,
        SdkErrorKind::Protocol
    );
}
