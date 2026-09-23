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
        let mut config: Self = toml::from_str(&text).map_err(|_| invalid())?;
        if config.cache_capacity > 16384
            || config.cache_ttl_seconds > 3600
            || config.minimum_interval_ms > 60000
            || config.queue_capacity > 4096
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
        Ok(config)
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
