use crate::{ClientError, ErrorKind};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};
use tonic::metadata::{MetadataMap, MetadataValue};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Clone, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub player_id: String,
    pub credential: String,
    pub device_id: Option<String>,
    pub bid: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub region: String,
    pub origin: String,
    pub allowed_origins: Vec<String>,
    pub platform: String,
    pub client_version: String,
    pub master_version: Option<String>,
    pub resource_version: Option<String>,
}

pub trait CredentialProvider: Send + Sync {
    fn credentials(&self, config: &SessionConfig) -> Result<Option<Credentials>, ClientError>;
}

pub struct StaticCredentials {
    region: String,
    origin: String,
    credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    region: String,
    origin: String,
    credentials: Credentials,
}

impl StaticCredentials {
    pub fn new(region: String, origin: String, credentials: Credentials) -> Self {
        Self {
            region,
            origin,
            credentials,
        }
    }

    /// Explicit read-only operator input. On Unix, group/other permissions are rejected.
    pub fn from_file(path: &Path) -> Result<Self, ClientError> {
        let file = std::fs::File::open(path).map_err(|_| bad_config())?;
        let meta = file.metadata().map_err(|_| bad_config())?;
        if !meta.is_file() || meta.len() > 64 * 1024 {
            return Err(bad_config());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(bad_config());
            }
        }
        use std::io::Read;
        let mut input = Zeroizing::new(String::new());
        file.take(64 * 1024 + 1)
            .read_to_string(&mut input)
            .map_err(|_| bad_config())?;
        if input.len() > 64 * 1024 {
            return Err(bad_config());
        }
        let parsed: CredentialFile = serde_json::from_str(&input).map_err(|_| bad_config())?;
        Ok(Self::new(parsed.region, parsed.origin, parsed.credentials))
    }
}

impl CredentialProvider for StaticCredentials {
    fn credentials(&self, config: &SessionConfig) -> Result<Option<Credentials>, ClientError> {
        if self.region != config.region || origin(&self.origin)? != origin(&config.origin)? {
            return Err(bad_config());
        }
        Ok(Some(self.credentials.clone()))
    }
}

impl SessionConfig {
    pub fn validate(&self) -> Result<(), ClientError> {
        let target = origin(&self.origin)?;
        if self.region.is_empty()
            || self.platform.is_empty()
            || self.client_version.is_empty()
            || !self
                .allowed_origins
                .iter()
                .any(|v| origin(v).ok().as_ref() == Some(&target))
            || self.master_version.is_some() != self.resource_version.is_some()
        {
            return Err(bad_config());
        }
        self.metadata(None, "validation", false)?;
        Ok(())
    }

    pub(crate) fn metadata(
        &self,
        credentials: Option<&Credentials>,
        request_id: &str,
        anonymous: bool,
    ) -> Result<MetadataMap, ClientError> {
        let mut result = MetadataMap::new();
        for (key, value) in [
            ("x-request-id", request_id),
            ("x-platform", &self.platform),
            ("x-client-version", &self.client_version),
        ] {
            insert(&mut result, key, value, false)?;
        }
        // Deliberately do not attach player credentials to anonymous support calls.
        if !anonymous && let Some(auth) = credentials {
            if auth.player_id.is_empty() || auth.credential.is_empty() {
                return Err(bad_config());
            }
            insert(&mut result, "x-player-id", &auth.player_id, true)?;
            insert(&mut result, "x-player-credential", &auth.credential, true)?;
            for (key, value) in [
                ("x-device-id", &auth.device_id),
                ("x-player-bid", &auth.bid),
            ] {
                if let Some(value) = value {
                    insert(&mut result, key, value, true)?;
                }
            }
            for (key, value) in [
                ("x-master-version", &self.master_version),
                ("x-resource-version", &self.resource_version),
            ] {
                if let Some(value) = value {
                    insert(&mut result, key, value, false)?;
                }
            }
        }
        Ok(result)
    }
}

fn insert(
    map: &mut MetadataMap,
    key: &'static str,
    value: &str,
    secret: bool,
) -> Result<(), ClientError> {
    let mut value = MetadataValue::try_from(value).map_err(|_| bad_config())?;
    value.set_sensitive(secret);
    map.insert(key, value);
    Ok(())
}

pub(crate) fn origin(value: &str) -> Result<String, ClientError> {
    let url = Url::parse(value).map_err(|_| bad_config())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(bad_config());
    }
    Ok(url.origin().ascii_serialization())
}

fn bad_config() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}
