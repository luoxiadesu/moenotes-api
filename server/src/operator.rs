//! Explicit operator authentication and scoped on-disk state, never an HTTP API.
use moenotes_client::{
    CancellationToken, Client, ClientError, ClientOptions, ErrorKind, QueryClient, SessionConfig,
    StaticCredentials,
    auth::{AndroidLoginContext, SdkAuthorization},
    sdk_http::{
        AppKey, CommonParameters, PasswordAccountType, PasswordLogin, SdkHeaders, SdkHttpClient,
        SdkHttpConfig, SdkHttpOptions, SdkReply,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginConfig {
    #[serde(default)]
    pub context_file: PathBuf,
    pub context: Option<Context>,
    pub sdk_http_file: Option<PathBuf>,
    pub sdk_http: Option<SdkConfig>,
    pub state_dir: PathBuf,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    device_model: String,
    operating_system: String,
    device_identifier: String,
    global_channel_id: u32,
    brand_id: u32,
    area_id: u32,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkConfig {
    base_url: String,
    allowed_base_urls: Vec<String>,
    app_key: crate::secret::SecretString,
    #[serde(default)]
    common: BTreeMap<String, Option<String>>,
    /// TOML has no null; explicitly list common parameters to omit on the wire.
    #[serde(default)]
    omit_common: Vec<String>,
    country_id: u32,
    #[serde(default)]
    one_sdk_version: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    region: String,
    origin: String,
    sdk_file: String,
    game_file: String,
}

fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}

pub(crate) fn private_read(path: &Path) -> Result<Zeroizing<Vec<u8>>, ClientError> {
    let before = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !before.is_file() {
        return Err(invalid());
    }
    let file = fs::File::open(path).map_err(|_| invalid())?;
    let meta = file.metadata().map_err(|_| invalid())?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(not(unix))]
    return Err(invalid());
    #[cfg(unix)]
    {
        let mut value = Zeroizing::new(Vec::new());
        file.take(65537)
            .read_to_end(&mut value)
            .map_err(|_| invalid())?;
        if value.len() > 65536 {
            return Err(invalid());
        }
        Ok(value)
    }
}
pub(crate) fn private_dir(path: &Path) -> Result<(), ClientError> {
    let meta = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !meta.is_dir() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(invalid())
    }
}
fn publish(path: &Path, bytes: &[u8]) -> Result<(), ClientError> {
    let parent = path.parent().ok_or_else(invalid)?;
    private_dir(parent)?;
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() {
            return Err(invalid());
        }
        let _ = private_read(path)?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| invalid())?;
    temp.write_all(bytes).map_err(|_| invalid())?;
    temp.as_file().sync_all().map_err(|_| invalid())?;
    temp.persist(path).map_err(|_| invalid())?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| invalid())?;
    Ok(())
}
impl LoginConfig {
    pub fn validate_sources(&self) -> Result<(), ClientError> {
        if self.context.is_some() == !self.context_file.as_os_str().is_empty()
            || self.sdk_http.is_some() && self.sdk_http_file.is_some()
            || self
                .sdk_http_file
                .as_ref()
                .is_some_and(|p| p.as_os_str().is_empty())
            || self.state_dir.as_os_str().is_empty()
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn has_sdk_http(&self) -> bool {
        self.sdk_http.is_some() || self.sdk_http_file.is_some()
    }
    fn sdk_client(&self) -> Result<(SdkHttpClient, u32), ClientError> {
        match (&self.sdk_http, &self.sdk_http_file) {
            (Some(config), None) => config.client(),
            (None, Some(path)) => {
                let bytes = private_read(path)?;
                let config: SdkConfig = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                config.client()
            }
            _ => Err(invalid()),
        }
    }
    pub fn lock(&self) -> Result<fs::File, ClientError> {
        private_dir(&self.state_dir)?;
        let path = self.state_dir.join("operator.lock");
        if let Ok(meta) = fs::symlink_metadata(&path)
            && !meta.is_file()
        {
            return Err(invalid());
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(|_| invalid())?;
        file.try_lock()
            .map_err(|_| ClientError::new(ErrorKind::QueueFull))?;
        Ok(file)
    }
    pub fn validate(&self, config: &SessionConfig) -> Result<(), ClientError> {
        self.validate_sources()?;
        private_dir(&self.state_dir)?;
        self.context()?;
        config.validate()?;
        if self.has_sdk_http() {
            let _ = self.sdk_client()?;
        }
        Ok(())
    }
    pub fn context(&self) -> Result<AndroidLoginContext, ClientError> {
        self.validate_sources()?;
        let c: Context = if let Some(context) = &self.context {
            context.clone()
        } else {
            let bytes = private_read(&self.context_file)?;
            serde_json::from_slice(&bytes).map_err(|_| invalid())?
        };
        if [&c.device_model, &c.operating_system, &c.device_identifier]
            .iter()
            .any(|v| v.is_empty() || v.len() > 1024)
        {
            return Err(invalid());
        }
        Ok(AndroidLoginContext {
            device_model: c.device_model,
            operating_system: c.operating_system,
            device_identifier: c.device_identifier,
            global_channel_id: c.global_channel_id,
            brand_id: c.brand_id,
            area_id: c.area_id,
        })
    }
    fn pointer(&self, config: &SessionConfig) -> Result<Pointer, ClientError> {
        let bytes = private_read(&self.state_dir.join("current.json"))?;
        let p: Pointer = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if p.region != config.region || p.origin != config.origin {
            return Err(invalid());
        }
        for value in [&p.sdk_file, &p.game_file] {
            if Path::new(value).components().count() != 1
                || !value.ends_with(".json")
                || value.contains(['/', '\\'])
                || value.starts_with('.')
            {
                return Err(invalid());
            }
        }
        Ok(p)
    }
    pub fn credentials(
        &self,
        config: &SessionConfig,
    ) -> Result<Option<StaticCredentials>, ClientError> {
        match fs::symlink_metadata(self.state_dir.join("current.json")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Ok(meta) if meta.is_file() => {}
            _ => return Err(invalid()),
        }
        let p = self.pointer(config)?;
        StaticCredentials::from_file(&self.state_dir.join(p.game_file)).map(Some)
    }
    pub fn approved_sdk(&self, config: &SessionConfig) -> Result<SdkAuthorization, ClientError> {
        let p = self.pointer(config)?;
        SdkAuthorization::from_file(config, &self.state_dir.join(p.sdk_file))
    }
    pub fn persist(
        &self,
        client: &Client,
        config: &SessionConfig,
        sdk: &SdkAuthorization,
    ) -> Result<(), ClientError> {
        private_dir(&self.state_dir)?;
        let generation = client.generation();
        let id = uuid::Uuid::new_v4();
        let p = Pointer {
            region: config.region.clone(),
            origin: config.origin.clone(),
            sdk_file: format!("sdk-{id}.json"),
            game_file: format!("game-{id}.json"),
        };
        sdk.save_to_file(&self.state_dir.join(&p.sdk_file))?;
        client.save_session(generation, &self.state_dir.join(&p.game_file))?;
        if client.generation() != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        publish(
            &self.state_dir.join("current.json"),
            &serde_json::to_vec(&p).map_err(|_| invalid())?,
        )
    }
}

impl SdkConfig {
    fn client(&self) -> Result<(SdkHttpClient, u32), ClientError> {
        let mut common = self.common.clone();
        for key in &self.omit_common {
            if common.insert(key.clone(), None).is_some() {
                return Err(invalid());
            }
        }
        let client = SdkHttpClient::new(
            SdkHttpConfig {
                base_url: self.base_url.clone(),
                allowed_base_urls: self.allowed_base_urls.clone(),
            },
            CommonParameters::new(common).map_err(|_| invalid())?,
            AppKey::new(self.app_key.0.clone()).map_err(|_| invalid())?,
            SdkHeaders {
                trace_id: None,
                one_sdk_version: self.one_sdk_version.clone(),
            },
            SdkHttpOptions {
                timeout: Duration::from_secs(30),
                ..Default::default()
            },
        )
        .map_err(|_| invalid())?;
        Ok((client, self.country_id))
    }
}

pub async fn sdk_login(
    config: &SessionConfig,
    login: &LoginConfig,
    email: String,
    password: Zeroizing<String>,
) -> Result<(), String> {
    login
        .validate(config)
        .map_err(|_| "invalid private login configuration")?;
    let _lock = login
        .lock()
        .map_err(|_| "another authentication operation is active")?;
    let sdk =
        password_authorization(config, login, email, password, CancellationToken::new()).await?;
    save_pending(login, &sdk)?;
    Ok(())
}

fn save_pending(login: &LoginConfig, sdk: &SdkAuthorization) -> Result<(), String> {
    let file = format!("pending-{}.json", uuid::Uuid::new_v4());
    sdk.save_to_file(&login.state_dir.join(&file))
        .map_err(|_| "SDK snapshot failed")?;
    publish(
        &login.state_dir.join("pending.json"),
        &serde_json::to_vec(&serde_json::json!({"file":file})).unwrap(),
    )
    .map_err(|_| "SDK pointer failed")?;
    Ok(())
}

pub(crate) async fn password_authorization(
    config: &SessionConfig,
    login: &LoginConfig,
    mut email: String,
    password: Zeroizing<String>,
    cancel: CancellationToken,
) -> Result<SdkAuthorization, String> {
    let (client, country_id) = login
        .sdk_client()
        .map_err(|_| "invalid SDK HTTP configuration")?;
    let rsa = match client.fetch_rsa(None, cancel.clone()).await {
        Ok(SdkReply::Success(value)) => value,
        Ok(SdkReply::Rejected(value)) => return Err(format!("SDK RSA rejected: {}", value.code())),
        Err(e) => return Err(format!("SDK RSA failed: {:?}", e.kind)),
    };
    let request = PasswordLogin {
        user_id: std::mem::take(&mut email),
        password: password.to_string(),
        account_type: PasswordAccountType::Email { country_id },
        ticket: None,
        third_payment_voucher: None,
    };
    let reply = client.password_login(&rsa, &request, None, cancel).await;
    drop(request);
    drop(password);
    let user = match reply {
        Ok(SdkReply::Success(user)) => user,
        Ok(SdkReply::Rejected(e)) => {
            return Err(format!(
                "SDK login rejected: {}; challenge={}, restore={}",
                e.code(),
                e.requires_challenge(),
                e.requires_account_restore()
            ));
        }
        Err(e) => return Err(format!("SDK login failed: {:?}", e.kind)),
    };
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Callback<'a> {
        uid: &'a str,
        access_token: &'a str,
        id_token: Option<&'a str>,
    }
    let bytes = Zeroizing::new(
        serde_json::to_vec(&Callback {
            uid: user.uid(),
            access_token: user.access_key(),
            id_token: user.id_token(),
        })
        .map_err(|_| "SDK serialization failed")?,
    );
    let sdk = SdkAuthorization::from_callback_json(config, &bytes)
        .map_err(|_| "SDK callback rejected")?;
    Ok(sdk)
}

pub async fn game_login(
    config: &SessionConfig,
    login: &LoginConfig,
    options: ClientOptions,
    allow_create: bool,
) -> Result<(), String> {
    login
        .validate(config)
        .map_err(|_| "invalid private login configuration")?;
    let _lock = login
        .lock()
        .map_err(|_| "another authentication operation is active")?;
    let raw = private_read(&login.state_dir.join("pending.json"))
        .map_err(|_| "pending SDK login required")?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pending {
        file: String,
    }
    let pending: Pending = serde_json::from_slice(&raw).map_err(|_| "invalid pending state")?;
    if !pending.file.starts_with("pending-")
        || !pending.file.ends_with(".json")
        || pending.file.contains(['/', '\\'])
    {
        return Err("invalid pending path".into());
    }
    let sdk = SdkAuthorization::from_file(config, &login.state_dir.join(pending.file))
        .map_err(|_| "invalid SDK snapshot")?;
    let client =
        Client::new(config.clone(), None, options).map_err(|_| "invalid session config")?;
    let context = login.context().map_err(|_| "invalid device context")?;
    if !client
        .pre_login_with_sdk(
            client.generation(),
            &sdk,
            &context,
            CancellationToken::new(),
        )
        .await
        .map_err(|e| format!("pre-login failed: {:?}", e.kind))?
        && !allow_create
    {
        return Err(
            "No role found. Re-run with --allow-create only if account creation is intended."
                .into(),
        );
    }
    client
        .login_with_sdk(
            client.generation(),
            &sdk,
            &context,
            CancellationToken::new(),
        )
        .await
        .map_err(|e| format!("game login failed: {:?}", e.kind))?;
    login
        .persist(&client, config, &sdk)
        .map_err(|_| "Session activated in memory but persistence failed; no automatic retry.")?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn setup() -> (tempfile::TempDir, LoginConfig, SessionConfig) {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let context = dir.path().join("context.json");
        fs::write(&context,r#"{"device_model":"synthetic","operating_system":"synthetic","device_identifier":"synthetic","global_channel_id":1,"brand_id":1,"area_id":1}"#).unwrap();
        fs::set_permissions(&context, fs::Permissions::from_mode(0o600)).unwrap();
        let login = LoginConfig {
            context_file: context,
            context: None,
            sdk_http_file: None,
            sdk_http: None,
            state_dir: dir.path().into(),
        };
        let config = SessionConfig {
            region: "test".into(),
            origin: "https://game.invalid".into(),
            allowed_origins: vec!["https://game.invalid".into()],
            platform: "android".into(),
            client_version: "1".into(),
            master_version: None,
            resource_version: None,
        };
        (dir, login, config)
    }
    #[tokio::test]
    async fn inline_sdk_omissions_match_legacy_json_nulls() {
        let value: toml::Value =
            toml::from_str(include_str!("../tests/fixtures/inline-config.toml")).unwrap();
        let inline: SdkConfig = value["login"]["sdk_http"].clone().try_into().unwrap();
        let mut legacy_common = inline.common.clone();
        legacy_common.insert("adid".into(), None);
        let legacy: SdkConfig = serde_json::from_value(serde_json::json!({
            "base_url":inline.base_url,"allowed_base_urls":inline.allowed_base_urls,
            "app_key":inline.app_key.0,"country_id":inline.country_id,"common":legacy_common,
        }))
        .unwrap();
        inline.client().unwrap();
        legacy.client().unwrap();
        let mut combined = inline.common.clone();
        for name in &inline.omit_common {
            combined.insert(name.clone(), None);
        }
        assert_eq!(combined, legacy.common);
        assert_eq!(combined["ad_ext"], Some(String::new()));
        assert_eq!(combined["adid"], None);
        for omit in [vec!["adid", "adid"], vec!["game_id"], vec!["unknown"]] {
            let mut invalid = inline.clone();
            invalid.omit_common = omit.into_iter().map(String::from).collect();
            assert!(invalid.client().is_err());
        }
    }
    #[tokio::test]
    async fn snapshots_roundtrip_and_pointer_cannot_escape() {
        let (dir, login, config) = setup();
        login.validate(&config).unwrap();
        assert!(login.credentials(&config).unwrap().is_none());
        let provider = StaticCredentials::new(
            config.region.clone(),
            config.origin.clone(),
            moenotes_client::Credentials {
                player_id: "synthetic-player".into(),
                credential: "synthetic-secret".into(),
                device_id: None,
                bid: Some("synthetic-sdk".into()),
            },
        );
        let client =
            Client::new(config.clone(), Some(&provider), ClientOptions::default()).unwrap();
        let sdk = SdkAuthorization::from_callback_json(
            &config,
            br#"{"uid":"synthetic-sdk","accessToken":"synthetic-token"}"#,
        )
        .unwrap();
        login.persist(&client, &config, &sdk).unwrap();
        let first = private_read(&dir.path().join("current.json")).unwrap();
        assert!(login.credentials(&config).unwrap().is_some());
        assert!(login.approved_sdk(&config).is_ok());
        login.persist(&client, &config, &sdk).unwrap();
        assert_ne!(
            &*first,
            &*private_read(&dir.path().join("current.json")).unwrap()
        );
        let mut pointer: Pointer = serde_json::from_slice(&first).unwrap();
        pointer.game_file = "../secret.json".into();
        publish(
            &dir.path().join("current.json"),
            &serde_json::to_vec(&pointer).unwrap(),
        )
        .unwrap();
        assert!(login.credentials(&config).is_err());
    }
    #[test]
    fn private_files_lock_and_atomic_pointer_validation() {
        let (dir, login, _) = setup();
        let lock = login.lock().unwrap();
        assert!(login.lock().is_err());
        drop(lock);
        assert!(login.lock().is_ok());
        let p = dir.path().join("pointer.json");
        publish(&p, b"one").unwrap();
        publish(&p, b"two").unwrap();
        assert_eq!(&*private_read(&p).unwrap(), b"two");
        let link = dir.path().join("link");
        symlink(&p, &link).unwrap();
        assert!(publish(&link, b"bad").is_err());
        assert!(private_read(&link).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"two");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_read(&p).is_err());
        assert!(publish(&p, b"bad").is_err());
    }
}
