//! Explicit GS SDK HTTP login primitives, not a complete SDK login workflow.
//! No HTTP gateway routes, automatic challenge handling, refresh or game login.
#![doc = include_str!("../../docs/sdk-http.md")]
mod form;
mod response;
pub use response::{PendingSdkLogin, RsaChallenge, SdkRejection, SdkReply};

use crate::{CancellationToken, Generation};
use base64::{Engine, engine::general_purpose::STANDARD};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs8::DecodePublicKey, traits::PublicKeyParts};
use std::{collections::BTreeMap, fmt, time::Duration};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const MAX_RESPONSE: usize = 256 * 1024;
pub const RSA_PATH: &str = "/gapi/client/rsa";
pub const PASSWORD_PATH: &str = "/gapi/client/login";
pub const CACHE_PATH: &str = "/gapi/client/cache.login";
pub const COMMON_FIELDS: &[&str] = &[
    "game_id",
    "server_id",
    "merchant_id",
    "app_ver",
    "sdk_ver",
    "channel_id",
    "platform",
    "platform_type",
    "net",
    "operators",
    "model",
    "pf_ver",
    "udid",
    "dp",
    "adid",
    "lang",
    "sdk_log_type",
    "ad_ext",
    "time_zone",
    "isRoot",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkErrorKind {
    InvalidConfig,
    InvalidInput,
    Protocol,
    Transport,
    HttpStatus,
    Timeout,
    Cancelled,
    Busy,
}

/// Never contains upstream bodies, URLs, password/token values or reqwest errors.
#[derive(Debug, thiserror::Error)]
#[error("SDK HTTP {kind:?}")]
pub struct SdkError {
    pub kind: SdkErrorKind,
    pub status: Option<u16>,
}
fn error(kind: SdkErrorKind) -> SdkError {
    SdkError { kind, status: None }
}
fn invalid() -> SdkError {
    error(SdkErrorKind::InvalidInput)
}
fn protocol() -> SdkError {
    error(SdkErrorKind::Protocol)
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AppKey(String);
impl AppKey {
    /// Explicit operator input, never extracted from an APK by this library.
    pub fn new(value: String) -> Result<Self, SdkError> {
        let key = Self(value);
        if key.0.is_empty() || key.0.len() > 4096 {
            return Err(invalid());
        }
        Ok(key)
    }
}

/// Caller-supplied SDK runtime values, not game gRPC metadata or inferred IDs.
pub struct CommonParameters(BTreeMap<String, Zeroizing<String>>);
impl CommonParameters {
    /// All 20 known keys must be explicit; None reproduces SDK null omission.
    /// Values have native addEncoded semantics, including '+' meaning a space.
    pub fn new(values: BTreeMap<String, Option<String>>) -> Result<Self, SdkError> {
        let values: BTreeMap<_, _> = values
            .into_iter()
            .map(|(k, v)| (k, v.map(Zeroizing::new)))
            .collect();
        if values.len() != COMMON_FIELDS.len()
            || !COMMON_FIELDS.iter().all(|k| values.contains_key(*k))
            || values.values().flatten().any(|v| v.len() > 4096)
        {
            return Err(invalid());
        }
        Ok(Self(
            values
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| (k, v)))
                .collect(),
        ))
    }
}

#[derive(Clone, Debug)]
pub struct SdkHttpConfig {
    /// Exact approved SDK login base, HTTPS root or /passport. No game origin fallback.
    pub base_url: String,
    pub allowed_base_urls: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SdkHttpOptions {
    /// Rate waiting and HTTP I/O deadline; excludes local RSA and JSON processing.
    pub timeout: Duration,
    /// Minimum time between admitted request starts. Concurrent calls return Busy.
    pub minimum_interval: Duration,
}
impl Default for SdkHttpOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            minimum_interval: Duration::from_secs(1),
        }
    }
}

/// Telemetry is explicit. No device identity is generated or discovered.
#[derive(Default, Zeroize, ZeroizeOnDrop)]
pub struct SdkHeaders {
    pub trace_id: Option<String>,
    pub one_sdk_version: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum PasswordAccountType {
    Legacy,
    Email { country_id: u32 },
    Phone { country_id: u32 },
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PasswordLogin {
    pub user_id: String,
    pub password: String,
    #[zeroize(skip)]
    pub account_type: PasswordAccountType,
    pub ticket: Option<String>,
    pub third_payment_voucher: Option<String>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CacheLogin {
    /// Native alreadyEncoded=true form input. The preserved key is this original string.
    pub access_key: String,
    pub ticket: Option<String>,
    pub third_payment_voucher: Option<String>,
    /// Optional current SDK user, matching NetworkUtil's conditional uid/mid insertion.
    pub current_user: Option<CurrentSdkUser>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CurrentSdkUser {
    pub uid: String,
    pub mid: String,
}

macro_rules! redact {
    ($($ty:ty),*) => {$ (
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )*};
}
redact!(
    AppKey,
    CommonParameters,
    SdkHeaders,
    PasswordLogin,
    CacheLogin,
    CurrentSdkUser
);

pub struct SdkHttpClient {
    config: SdkHttpConfig,
    parameters: CommonParameters,
    key: AppKey,
    headers: SdkHeaders,
    http: reqwest::Client,
    options: SdkHttpOptions,
    admission: Semaphore,
    next: Mutex<Instant>,
    binding: Generation,
}

fn normalize_base(value: &str) -> Result<String, SdkError> {
    let url = Url::parse(value).map_err(|_| error(SdkErrorKind::InvalidConfig))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/" | "/passport" | "/passport/")
    {
        return Err(error(SdkErrorKind::InvalidConfig));
    }
    Ok(format!(
        "{}{}",
        url.origin().ascii_serialization(),
        url.path().trim_end_matches('/')
    ))
}

impl SdkHttpClient {
    pub fn new(
        config: SdkHttpConfig,
        parameters: CommonParameters,
        key: AppKey,
        headers: SdkHeaders,
        options: SdkHttpOptions,
    ) -> Result<Self, SdkError> {
        let base = normalize_base(&config.base_url)?;
        if !config
            .allowed_base_urls
            .iter()
            .any(|v| normalize_base(v).ok().as_ref() == Some(&base))
            || options.timeout.is_zero()
            || options.timeout > Duration::from_secs(120)
            || options.minimum_interval > Duration::from_secs(120)
        {
            return Err(error(SdkErrorKind::InvalidConfig));
        }
        for value in [&headers.trace_id, &headers.one_sdk_version]
            .into_iter()
            .flatten()
        {
            if value.len() > 1024 || reqwest::header::HeaderValue::from_str(value).is_err() {
                return Err(error(SdkErrorKind::InvalidConfig));
            }
        }
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(options.timeout)
            .build()
            .map_err(|_| error(SdkErrorKind::InvalidConfig))?;
        Ok(Self {
            config: SdkHttpConfig {
                base_url: base,
                ..config
            },
            parameters,
            key,
            headers,
            http,
            options,
            admission: Semaphore::new(1),
            next: Mutex::new(Instant::now()),
            binding: Generation::new_v4(),
        })
    }

    /// One signed POST. RSA key and hash stay bound to this immutable client instance.
    pub async fn fetch_rsa(
        &self,
        current_user: Option<&CurrentSdkUser>,
        cancel: CancellationToken,
    ) -> Result<SdkReply<RsaChallenge>, SdkError> {
        let body = self
            .send(RSA_PATH, Vec::new(), true, current_user, cancel)
            .await?;
        response::rsa(&body, self.binding)
    }

    /// One password RPC using a previously fetched RSA challenge. No automatic fetch/retry.
    /// SDK consent/initialization requirements are not implemented by this primitive.
    pub async fn password_login(
        &self,
        rsa: &RsaChallenge,
        login: &PasswordLogin,
        current_user: Option<&CurrentSdkUser>,
        cancel: CancellationToken,
    ) -> Result<SdkReply<PendingSdkLogin>, SdkError> {
        if cancel.is_cancelled() {
            return Err(error(SdkErrorKind::Cancelled));
        }
        if rsa.binding != self.binding {
            return Err(invalid());
        }
        validate_secret(&login.user_id, 1024)?;
        validate_secret(&login.password, 4096)?;
        let encrypted = encrypt_password(rsa, &login.password)?;
        let mut fields = vec![
            ("user_id".into(), Zeroizing::new(login.user_id.clone())),
            ("pwd".into(), encrypted),
        ];
        optional(&mut fields, "ticket", &login.ticket)?;
        optional(
            &mut fields,
            "third_payment_voucher",
            &login.third_payment_voucher,
        )?;
        match login.account_type {
            PasswordAccountType::Legacy => {}
            PasswordAccountType::Email { country_id }
            | PasswordAccountType::Phone { country_id } => {
                fields.push(("country_id".into(), Zeroizing::new(country_id.to_string())));
                fields.push((
                    "user_id_type".into(),
                    Zeroizing::new(
                        if matches!(login.account_type, PasswordAccountType::Email { .. }) {
                            "0"
                        } else {
                            "1"
                        }
                        .into(),
                    ),
                ));
            }
        }
        let body = self
            .send(PASSWORD_PATH, fields, false, current_user, cancel)
            .await?;
        response::login(&body, None)
    }

    /// One cache-login RPC; success retains the submitted access key, not a rotation.
    pub async fn cache_login(
        &self,
        login: &CacheLogin,
        cancel: CancellationToken,
    ) -> Result<SdkReply<PendingSdkLogin>, SdkError> {
        validate_secret(&login.access_key, 16 * 1024)?;
        let mut fields = vec![(
            "access_key".into(),
            Zeroizing::new(login.access_key.clone()),
        )];
        optional(&mut fields, "ticket", &login.ticket)?;
        optional(
            &mut fields,
            "third_payment_voucher",
            &login.third_payment_voucher,
        )?;
        let body = self
            .send(
                CACHE_PATH,
                fields,
                true,
                login.current_user.as_ref(),
                cancel,
            )
            .await?;
        response::login(&body, Some(&login.access_key))
    }

    fn form(
        &self,
        mut fields: Vec<(String, Zeroizing<String>)>,
        already_encoded: bool,
        current_user: Option<&CurrentSdkUser>,
        timestamp_ms: u64,
    ) -> Result<Zeroizing<String>, SdkError> {
        if let Some(user) = current_user {
            validate_secret(&user.uid, 256)?;
            if user.mid.len() > 256 {
                return Err(invalid());
            }
            fields.push(("uid".into(), Zeroizing::new(user.uid.clone())));
            fields.push(("mid".into(), Zeroizing::new(user.mid.clone())));
        }
        let mut fields = fields
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    if already_encoded {
                        form::encoded(&v)
                    } else {
                        form::raw(&v)
                    },
                )
            })
            .collect::<Vec<_>>();
        fields.extend(
            self.parameters
                .0
                .iter()
                .map(|(k, v)| (k.clone(), form::encoded(v))),
        );
        fields.push(("timestamp".into(), Zeroizing::new(timestamp_ms.to_string())));
        fields.push(("web_code".into(), Zeroizing::new("6".into())));
        form::sign_form(fields, &self.key.0)
    }

    async fn send(
        &self,
        path: &str,
        fields: Vec<(String, Zeroizing<String>)>,
        already_encoded: bool,
        current_user: Option<&CurrentSdkUser>,
        cancel: CancellationToken,
    ) -> Result<Zeroizing<Vec<u8>>, SdkError> {
        let _permit = self
            .admission
            .try_acquire()
            .map_err(|_| error(SdkErrorKind::Busy))?;
        let work = async {
            let mut next = self.next.lock().await;
            tokio::time::sleep_until(*next).await;
            *next = Instant::now() + self.options.minimum_interval;
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_millis();
            let body = self.form(
                fields,
                already_encoded,
                current_user,
                u64::try_from(ms).map_err(|_| invalid())?,
            )?;
            let mut request = self
                .http
                .post(format!("{}{path}", self.config.base_url))
                .header("User-Agent", "Mozilla/5.0 BSGameSDK")
                .header("Api-Version", "1")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("X-Game-Request-Id", Generation::new_v4().to_string());
            for (name, value) in [
                ("X-Game-Trace-Id", &self.headers.trace_id),
                ("one-sdk-ver", &self.headers.one_sdk_version),
            ] {
                if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                    let mut value =
                        reqwest::header::HeaderValue::from_str(value).map_err(|_| invalid())?;
                    value.set_sensitive(true);
                    request = request.header(name, value);
                }
            }
            let mut response = request
                .body(body.as_bytes().to_vec())
                .send()
                .await
                .map_err(network_error)?;
            // HttpUtils.httpOk explicitly excludes no-content/reset-content responses.
            if !response.status().is_success() || matches!(response.status().as_u16(), 204 | 205) {
                return Err(SdkError {
                    kind: SdkErrorKind::HttpStatus,
                    status: Some(response.status().as_u16()),
                });
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE as u64)
            {
                return Err(protocol());
            }
            let mut bytes = Zeroizing::new(Vec::new());
            while let Some(chunk) = response.chunk().await.map_err(network_error)? {
                if bytes.len() + chunk.len() > MAX_RESPONSE {
                    return Err(protocol());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(error(SdkErrorKind::Cancelled)),
            result = tokio::time::timeout(self.options.timeout, work) => result.unwrap_or_else(|_| Err(error(SdkErrorKind::Timeout))),
        }
    }
}

fn network_error(err: reqwest::Error) -> SdkError {
    error(if err.is_timeout() {
        SdkErrorKind::Timeout
    } else {
        SdkErrorKind::Transport
    })
}
fn validate_secret(value: &str, max: usize) -> Result<(), SdkError> {
    if value.is_empty() || value.len() > max {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn optional(
    fields: &mut Vec<(String, Zeroizing<String>)>,
    name: &str,
    value: &Option<String>,
) -> Result<(), SdkError> {
    if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
        validate_secret(value, 16 * 1024)?;
        fields.push((name.into(), Zeroizing::new(value.clone())));
    }
    Ok(())
}
fn encrypt_password(
    challenge: &RsaChallenge,
    password: &str,
) -> Result<Zeroizing<String>, SdkError> {
    let key = RsaPublicKey::from_public_key_pem(&challenge.pem).map_err(|_| protocol())?;
    // Public-key encryption only. No RSA private-key operation in production.
    if !(1024..=4096).contains(&key.n().bits()) {
        return Err(protocol());
    }
    let plaintext = Zeroizing::new(format!("{}{}", challenge.hash.as_str(), password));
    if plaintext.len() > key.size() - 11 {
        return Err(invalid());
    }
    let ciphertext = key
        .encrypt(
            &mut rand::rngs::OsRng,
            Pkcs1v15Encrypt,
            plaintext.as_bytes(),
        )
        .map_err(|_| protocol())?;
    Ok(Zeroizing::new(STANDARD.encode(ciphertext)))
}

#[cfg(test)]
mod tests;
