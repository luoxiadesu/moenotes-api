use crate::{operator::LoginConfig, projection::ResponseMode};
use moenotes_client::{ClientError, ClientOptions, ErrorKind, SessionConfig};
use serde::Deserialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub enable_experimental_raw: bool,
    pub response_mode: Option<ResponseMode>,
    #[serde(default = "yes")]
    pub access_log: bool,
    pub login: Option<LoginConfig>,
    pub accounts: Option<crate::accounts::AccountsConfig>,
    #[serde(default)]
    pub recovery: RecoveryConfig,
    pub api_key_file: PathBuf,
    pub credentials_file: Option<PathBuf>,
    pub session: SessionConfig,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "interval")]
    pub minimum_interval_ms: u64,
    #[serde(default = "capacity")]
    pub queue_capacity: usize,
    #[serde(default = "ttl")]
    pub cache_ttl_seconds: u64,
    #[serde(default = "entries")]
    pub cache_capacity: usize,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "cooldown")]
    pub cooldown_seconds: u64,
}
fn cooldown() -> u64 {
    300
}
fn yes() -> bool {
    true
}
impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cooldown_seconds: cooldown(),
        }
    }
}
fn listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
}
fn timeout() -> u64 {
    10
}
fn interval() -> u64 {
    1000
}
fn capacity() -> usize {
    32
}
fn ttl() -> u64 {
    15
}
fn entries() -> usize {
    1024
}

impl Config {
    pub fn read(path: &Path) -> Result<Self, ClientError> {
        let text = std::fs::read_to_string(path).map_err(|_| invalid())?;
        Self::from_text(&text, path)
    }

    pub(crate) fn from_text(text: &str, path: &Path) -> Result<Self, ClientError> {
        let mut config: Self = toml::from_str(text).map_err(|_| invalid())?;
        if config.cache_capacity > 16384
            || config.cache_ttl_seconds > 3600
            || config.minimum_interval_ms > 60000
            || config.queue_capacity > 4096
            || config.timeout_seconds == 0
            || config.timeout_seconds > 120
            || config.recovery.cooldown_seconds < 60
            || config.recovery.cooldown_seconds > 86400
            || config.recovery.enabled && config.login.is_none()
            || config.login.is_some() && config.credentials_file.is_some()
            || config.accounts.is_some() && config.login.is_none()
            || config.accounts.is_some()
                && config
                    .login
                    .as_ref()
                    .is_some_and(|l| l.sdk_http_file.is_none())
            || config.enable_experimental_raw
                && config.response_mode.is_some_and(|m| m != ResponseMode::Raw)
        {
            return Err(invalid());
        }
        config.session.validate()?;
        let parent = path.parent().unwrap_or(Path::new("."));
        if config.api_key_file.is_relative() {
            config.api_key_file = parent.join(&config.api_key_file);
        }
        if let Some(file) = &mut config.credentials_file
            && file.is_relative()
        {
            *file = parent.join(&*file);
        }
        if let Some(login) = &mut config.login {
            if login.context_file.is_relative() {
                login.context_file = parent.join(&login.context_file);
            }
            if login.state_dir.is_relative() {
                login.state_dir = parent.join(&login.state_dir);
            }
            if let Some(file) = &mut login.sdk_http_file
                && file.is_relative()
            {
                *file = parent.join(&*file);
            }
        }
        if let Some(accounts) = &mut config.accounts {
            accounts.validate()?;
            if accounts.directory.is_relative() {
                accounts.directory = parent.join(&accounts.directory);
            }
        }
        Ok(config)
    }

    pub fn mode(&self) -> ResponseMode {
        self.response_mode
            .unwrap_or(if self.enable_experimental_raw {
                ResponseMode::Raw
            } else {
                ResponseMode::Public
            })
    }

    pub fn client_options(&self) -> ClientOptions {
        ClientOptions {
            timeout: Duration::from_secs(self.timeout_seconds),
            minimum_interval: Duration::from_millis(self.minimum_interval_ms),
            queue_capacity: self.queue_capacity,
        }
    }
}

pub fn read_api_key(path: &Path) -> Result<Zeroizing<String>, ClientError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    let mut value = Zeroizing::new(String::new());
    file.take(4097)
        .read_to_string(&mut value)
        .map_err(|_| invalid())?;
    if value.len() > 4096 {
        return Err(invalid());
    }
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}
fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}
