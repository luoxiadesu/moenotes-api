use super::*;
use crate::auth::{AndroidLoginContext, LOGIN, PRE_LOGIN, SdkAuthorization};

fn sdk() -> SdkAuthorization {
    SdkAuthorization::from_callback_json(
        &config(),
        br#"{
        "uid":"synthetic-sdk-uid", "accessToken":"synthetic-sdk-token",
        "idToken":"synthetic-id-token", "channelToken":"unused-secret",
        "channelId":"not-a-number", "userState":1, "isSwitchedAccount":true
    }"#,
    )
    .unwrap()
}

fn context() -> AndroidLoginContext {
    AndroidLoginContext {
        device_model: "synthetic-model".into(),
        operating_system: "synthetic-os".into(),
        device_identifier: "synthetic-identifier".into(),
        global_channel_id: 12,
        brand_id: 34,
        area_id: 56,
    }
}

#[test]
fn callback_import_is_bounded_and_redacted() {
    assert_eq!(format!("{:?}", sdk()), "SdkAuthorization([REDACTED])");
    assert!(!format!("{:?}", context()).contains("synthetic"));
    for bad in [
        "null",
        "[]",
        "{}",
        r#"{"uid":1,"accessToken":"x"}"#,
        r#"{"uid":"x","accessToken":""}"#,
        r#"{"uid":"x","accessToken":null}"#,
        r#"{"uid":"x","uid":"y","accessToken":"x"}"#,
        r#"{"uid":"x\ny","accessToken":"x"}"#,
        r#"{"uid":"x","accessToken":"x","idToken":5}"#,
    ] {
        let err = SdkAuthorization::from_callback_json(&config(), bad.as_bytes()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest);
        assert!(!format!("{err:?}").contains("accessToken"));
    }
    assert!(SdkAuthorization::from_callback_json(&config(), &vec![b' '; 65537]).is_err());
    for extra in ["", r#", "idToken":null"#, r#", "idToken":"""#] {
        let json = format!(r#"{{"uid":"x","accessToken":"x"{extra}}}"#);
        assert!(SdkAuthorization::from_callback_json(&config(), json.as_bytes()).is_ok());
    }
    let mut ios = config();
    ios.platform = "ios".into();
    assert!(
        SdkAuthorization::from_callback_json(&ios, br#"{"uid":"x","accessToken":"x"}"#).is_err()
    );
}

#[cfg(unix)]
#[test]
fn sdk_cache_is_private_bound_and_never_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("sdk.json");
    sdk().save_to_file(&path).unwrap();
    let restored = SdkAuthorization::from_file(&config(), &path).unwrap();
    assert!(!format!("{restored:?}").contains("synthetic"));
    assert!(restored.save_to_file(&path).is_err());
    let mut other = config();
    other.origin = "https://other.invalid".into();
    other.allowed_origins = vec![other.origin.clone()];
    assert!(SdkAuthorization::from_file(&other, &path).is_err());
    other = config();
    other.region = "other".into();
    assert!(SdkAuthorization::from_file(&other, &path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SdkAuthorization::from_file(&config(), &path).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn game_session_export_roundtrips_and_checks_generation() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::with_transport(config(), None, options(), reply(success())).unwrap();
    let path = dir.path().join("game.json");
    assert_eq!(
        client
            .save_session(client.generation(), &path)
            .unwrap_err()
            .kind,
        ErrorKind::AuthenticationRequired
    );
    let old = client.generation();
    let receipt = client
        .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        client.save_session(old, &path).unwrap_err().kind,
        ErrorKind::SessionChanged
    );
    client.save_session(receipt.generation, &path).unwrap();
    let restored = StaticCredentials::from_file(&path)
        .unwrap()
        .credentials(&config())
        .unwrap()
        .unwrap();
    assert_eq!(restored.player_id, "synthetic-new-player");
    assert_eq!(restored.credential, "synthetic-new-secret");
    assert_eq!(restored.bid.as_deref(), Some("synthetic-sdk-uid"));
    assert_eq!(restored.device_id, None);
    let provider = StaticCredentials::from_file(&path).unwrap();
    assert!(
        client
            .matches_identity(receipt.generation, &provider)
            .unwrap()
    );
    let other = StaticCredentials::new(
        config().region,
        config().origin,
        Credentials {
            player_id: "another-player".into(),
            credential: "x".into(),
            device_id: None,
            bid: Some("synthetic-sdk-uid".into()),
        },
    );
    assert!(!client.matches_identity(receipt.generation, &other).unwrap());
    assert!(client.save_session(receipt.generation, &path).is_err());
    *client.session.read().unwrap().blocked.write().unwrap() = Some(ErrorKind::Authentication);
    assert_eq!(
        client
            .save_session(receipt.generation, &dir.path().join("blocked.json"))
            .unwrap_err()
            .kind,
        ErrorKind::Authentication
    );
    assert!(!dir.path().join("blocked.json").exists());
}

#[tokio::test]
async fn login_and_prelogin_over_local_grpc() {
    let calls: Calls = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let router = Router::new().fallback(mock_grpc).with_state(calls.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap()
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let client = Client::with_transport(
        config(),
        Some(&credentials()),
        options(),
        Arc::new(mock_channel(channel)),
    )
    .unwrap();
    let old = client.generation();
    assert!(
        client
            .pre_login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
            .await
            .unwrap()
    );
    assert_eq!(client.generation(), old);
    let receipt = client
        .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(receipt.is_new_user, 7);
    assert_ne!(receipt.generation, old);
    assert_eq!(client.generation(), receipt.generation);
    assert_eq!(
        client.session_status(),
        SessionStatus::CredentialsUnverified
    );
    assert_eq!(
        client
            .execute(
                old,
                Query::Whoami(Default::default()),
                CancellationToken::new()
            )
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::SessionChanged
    );
    client
        .query(Query::Whoami(Default::default()))
        .await
        .unwrap();
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for (method, bytes, headers) in &calls[..2] {
            assert!(*method == LOGIN || *method == PRE_LOGIN);
            let request =
                generated::app::playerlogin::PlayerLoginRequest::decode(bytes.as_slice()).unwrap();
            assert_eq!(request.sdk_uid, "synthetic-sdk-uid");
            assert_eq!(request.sdk_access_token, "synthetic-sdk-token");
            assert_eq!(
                request.id_token,
                if *method == LOGIN {
                    "synthetic-id-token"
                } else {
                    ""
                }
            );
            assert_eq!(request.platform, 0);
            assert_eq!(request.client_version, "1.0.1");
            assert_eq!(request.client_package, "com.bilibili.sirius");
            assert_eq!(request.device_model, "synthetic-model");
            assert_eq!(request.operating_system, "synthetic-os");
            assert_eq!(
                (request.global_channel_id, request.brand_id, request.area_id),
                (12, 34, 56)
            );
            assert!(request.initial_data_group.is_empty());
            let uuid = request.uuid.unwrap();
            assert!(uuid.ad_id.is_empty());
            assert_eq!(uuid.identifier, "synthetic-identifier");
            for header in [
                "x-player-id",
                "x-player-credential",
                "x-device-id",
                "x-master-version",
                "x-resource-version",
            ] {
                assert!(headers.get(header).is_none(), "{header}");
            }
            assert_eq!(headers.get("x-player-bid").unwrap(), "synthetic-sdk-uid");
            assert!(headers.get("x-request-id").is_some());
            assert_eq!(headers.get("x-client-version").is_some(), *method == LOGIN);
            assert!(
                !headers
                    .keys()
                    .any(|key| format!("{key:?}").contains("override"))
            );
        }
        let (_, _, headers) = &calls[2];
        assert_eq!(headers.get("x-player-id").unwrap(), "synthetic-new-player");
        assert_eq!(
            headers.get("x-player-credential").unwrap(),
            "synthetic-new-secret"
        );
        assert_eq!(headers.get("x-player-bid").unwrap(), "synthetic-sdk-uid");
        assert!(headers.get("x-device-id").is_none());
        assert_eq!(headers.get("x-master-version").unwrap(), "master-test");
    }
    assert!(Query::from_json(LOGIN.name, serde_json::json!({})).is_err());
    assert!(!METHODS.contains(&LOGIN));
    drop(client);
    stop.cancel();
    server.await.unwrap();
}

struct AuthReply {
    calls: AtomicUsize,
    value: serde_json::Value,
    error: Option<ErrorKind>,
    gate: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait]
impl Transport for AuthReply {
    async fn call(
        &self,
        method: Method,
        _: DynamicMessage,
        headers: MetadataMap,
        _: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(headers.get("x-player-bid").unwrap().is_sensitive());
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        if let Some(error) = self.error {
            return Err(ClientError::new(error));
        }
        DynamicMessage::deserialize(
            pool().get_message_by_name(method.output).unwrap(),
            self.value.clone(),
        )
        .map_err(|_| ClientError::new(ErrorKind::Protocol))
    }
}
fn reply(value: serde_json::Value) -> Arc<AuthReply> {
    Arc::new(AuthReply {
        calls: AtomicUsize::new(0),
        value,
        error: None,
        gate: None,
    })
}
fn success() -> serde_json::Value {
    serde_json::json!({"credential":{"id":"synthetic-new-player","credential":"synthetic-new-secret"}})
}

#[tokio::test]
async fn explicit_login_recovers_authentication_rejection() {
    let transport = reply(success());
    let client =
        Client::with_transport(config(), Some(&credentials()), options(), transport).unwrap();
    *client.session.read().unwrap().blocked.write().unwrap() = Some(ErrorKind::Authentication);
    let old = client.generation();
    let receipt = client
        .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
        .await
        .unwrap();
    assert_ne!(receipt.generation, old);
    assert_eq!(
        client.session_status(),
        SessionStatus::CredentialsUnverified
    );
}

#[tokio::test]
async fn anonymous_errors_cannot_weaken_device_or_version_blocks() {
    for kind in [ErrorKind::DeviceConflict, ErrorKind::Version] {
        let transport = Arc::new(Rejected {
            calls: AtomicUsize::new(0),
        });
        let client = Client::with_transport(config(), None, options(), transport.clone()).unwrap();
        *client.session.read().unwrap().blocked.write().unwrap() = Some(kind);
        assert_eq!(
            client
                .query(Query::Version(Default::default()))
                .await
                .err()
                .unwrap()
                .kind,
            ErrorKind::Authentication
        );
        assert_eq!(
            client
                .login_with_sdk(
                    client.generation(),
                    &sdk(),
                    &context(),
                    CancellationToken::new()
                )
                .await
                .unwrap_err()
                .kind,
            kind
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn malformed_login_cannot_replace_existing_session() {
    for value in [
        serde_json::json!({}),
        serde_json::json!({"credential":{}}),
        serde_json::json!({"credential":{"id":"synthetic-player","credential":"invalid\nheader"}}),
    ] {
        let transport = reply(value);
        let client =
            Client::with_transport(config(), Some(&credentials()), options(), transport.clone())
                .unwrap();
        let old = client.generation();
        let err = client
            .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Protocol);
        assert_eq!(client.generation(), old);
        assert_eq!(
            client
                .session
                .read()
                .unwrap()
                .credentials
                .as_ref()
                .unwrap()
                .credential,
            "synthetic-private-value"
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn auth_binding_rejects_other_region_origin_and_invalid_context() {
    for field in ["region", "origin", "platform", "context"] {
        let mut cfg = config();
        let mut ctx = context();
        match field {
            "region" => cfg.region = "other".into(),
            "origin" => {
                cfg.origin = "https://other.invalid".into();
                cfg.allowed_origins.push(cfg.origin.clone());
            }
            "platform" => cfg.platform = "ios".into(),
            _ => ctx.device_identifier.clear(),
        }
        let transport = reply(success());
        let client = Client::with_transport(cfg, None, options(), transport.clone()).unwrap();
        assert!(
            client
                .login_with_sdk(client.generation(), &sdk(), &ctx, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn login_errors_are_single_shot_and_device_version_blocks_persist() {
    for kind in [
        ErrorKind::Authentication,
        ErrorKind::Maintenance,
        ErrorKind::Business,
        ErrorKind::DeviceConflict,
        ErrorKind::Version,
    ] {
        let transport = Arc::new(AuthReply {
            calls: AtomicUsize::new(0),
            value: success(),
            error: Some(kind),
            gate: None,
        });
        let client = Client::with_transport(config(), None, options(), transport.clone()).unwrap();
        let old = client.generation();
        assert_eq!(
            client
                .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            kind
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.generation(), old);
        if matches!(kind, ErrorKind::DeviceConflict | ErrorKind::Version) {
            assert_eq!(
                client
                    .login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
                    .await
                    .unwrap_err()
                    .kind,
                kind
            );
            assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn login_cancel_timeout_and_replacement_never_install_stale_credentials() {
    for mode in ["cancel", "timeout", "replace"] {
        let transport = Arc::new(AuthReply {
            calls: AtomicUsize::new(0),
            value: success(),
            error: None,
            gate: Some(Arc::new(tokio::sync::Notify::new())),
        });
        let client = Arc::new(
            Client::with_transport(
                config(),
                None,
                ClientOptions {
                    timeout: Duration::from_millis(100),
                    ..options()
                },
                transport.clone(),
            )
            .unwrap(),
        );
        let old = client.generation();
        let token = CancellationToken::new();
        let cancel = token.clone();
        let c = client.clone();
        let pending =
            tokio::spawn(async move { c.login_with_sdk(old, &sdk(), &context(), cancel).await });
        while transport.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let expected = match mode {
            "cancel" => {
                token.cancel();
                ErrorKind::Cancelled
            }
            "replace" => {
                client.replace_session(config(), None).unwrap();
                ErrorKind::SessionChanged
            }
            _ => ErrorKind::Timeout,
        };
        assert_eq!(pending.await.unwrap().unwrap_err().kind, expected);
        assert_eq!(client.session_status(), SessionStatus::Anonymous);
        assert!(client.session.read().unwrap().credentials.is_none());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn successful_login_cancels_competing_old_generation_login() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let transport = Arc::new(AuthReply {
        calls: AtomicUsize::new(0),
        value: success(),
        error: None,
        gate: Some(gate.clone()),
    });
    let client =
        Arc::new(Client::with_transport(config(), None, options(), transport.clone()).unwrap());
    let old = client.generation();
    let c = client.clone();
    let first = tokio::spawn(async move {
        c.login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
            .await
    });
    while transport.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let c = client.clone();
    let second = tokio::spawn(async move {
        c.login_with_sdk(old, &sdk(), &context(), CancellationToken::new())
            .await
    });
    while client.admission.available_permits() != client.options.queue_capacity - 1 {
        tokio::task::yield_now().await;
    }
    gate.notify_one();
    assert!(first.await.unwrap().is_ok());
    assert_eq!(
        second.await.unwrap().unwrap_err().kind,
        ErrorKind::SessionChanged
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}
