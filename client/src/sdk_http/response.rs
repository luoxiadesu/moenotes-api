use super::{Generation, MAX_RESPONSE, SdkError, protocol, validate_secret};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Debug)]
pub enum SdkReply<T> {
    Success(T),
    Rejected(SdkRejection),
}

#[derive(Deserialize)]
struct Envelope<'a> {
    code: i32,
    #[serde(borrow)]
    data: Option<&'a RawValue>,
}

/// Code is safe to log; failure data can contain tokens and untrusted redirect URLs.
pub struct SdkRejection {
    code: i32,
    data: Zeroizing<String>,
}
impl SdkRejection {
    pub fn code(&self) -> i32 {
        self.code
    }
    /// 200007 is the recovered CAPTCHA branch. Never automatically opens/fetches URLs.
    pub fn requires_challenge(&self) -> bool {
        self.code == 200007
    }
    /// 800011 enters an account-restore confirmation, not an automatic retry.
    pub fn requires_account_restore(&self) -> bool {
        self.code == 800011
    }
    /// Explicit sensitive access for an integration's challenge UI. Not log-safe.
    /// Raw wire JSON is retained, not Gson's coercion into Map<String,String>.
    pub fn sensitive_data_json(&self) -> &str {
        &self.data
    }
}
impl fmt::Debug for SdkRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SdkRejection")
            .field("code", &self.code)
            .field("data", &"[REDACTED]")
            .finish()
    }
}

pub struct RsaChallenge {
    pub(super) pem: Zeroizing<String>,
    pub(super) hash: Zeroizing<String>,
    pub(super) binding: Generation,
}
impl fmt::Debug for RsaChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RsaChallenge([REDACTED])")
    }
}

/// HTTP success only. Native SDK still runs agreement/other post-login checks.
/// Deliberately has no automatic conversion to SdkAuthorization or game session.
#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct PendingSdkLogin {
    uid: String,
    #[serde(default)]
    access_key: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
}
impl PendingSdkLogin {
    /// Account-specific identifier, never a game player ID.
    pub fn uid(&self) -> &str {
        &self.uid
    }
    /// Sensitive value, for explicit integration use only. Never log it.
    pub fn access_key(&self) -> &str {
        self.access_key.as_deref().unwrap_or_default()
    }
    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref()
    }
    /// Raw field only. Its units/epoch and server lifetime policy are not inferred.
    pub fn raw_expires(&self) -> Option<i64> {
        self.expires
    }
    /// Presence does not establish a working refresh protocol.
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token.as_ref().is_some_and(|v| !v.is_empty())
    }
}
impl fmt::Debug for PendingSdkLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingSdkLogin([REDACTED])")
    }
}

fn parse<T>(
    bytes: &[u8],
    success: impl FnOnce(&RawValue) -> Result<T, SdkError>,
) -> Result<SdkReply<T>, SdkError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(protocol());
    }
    let envelope: Envelope<'_> = serde_json::from_slice(bytes).map_err(|_| protocol())?;
    if envelope.code != 0 {
        return Ok(SdkReply::Rejected(SdkRejection {
            code: envelope.code,
            data: Zeroizing::new(envelope.data.map_or("null", RawValue::get).to_owned()),
        }));
    }
    Ok(SdkReply::Success(success(
        envelope.data.ok_or_else(protocol)?,
    )?))
}

pub(super) fn rsa(bytes: &[u8], binding: Generation) -> Result<SdkReply<RsaChallenge>, SdkError> {
    #[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
    struct Data {
        rsa_key: String,
        hash: String,
    }
    parse(bytes, |raw| {
        let mut data: Data = serde_json::from_str(raw.get()).map_err(|_| protocol())?;
        if data.rsa_key.len() > 8192 || data.hash.len() > 1024 {
            return Err(protocol());
        }
        use rsa::{pkcs8::DecodePublicKey, traits::PublicKeyParts};
        let key = rsa::RsaPublicKey::from_public_key_pem(&data.rsa_key).map_err(|_| protocol())?;
        if !(1024..=4096).contains(&key.n().bits()) {
            return Err(protocol());
        }
        Ok(RsaChallenge {
            pem: Zeroizing::new(std::mem::take(&mut data.rsa_key)),
            hash: Zeroizing::new(std::mem::take(&mut data.hash)),
            binding,
        })
    })
}

pub(super) fn login(
    bytes: &[u8],
    old_key: Option<&str>,
) -> Result<SdkReply<PendingSdkLogin>, SdkError> {
    parse(bytes, |raw| {
        let mut user: PendingSdkLogin = serde_json::from_str(raw.get()).map_err(|_| protocol())?;
        if let Some(old) = old_key {
            // CacheLoginActivity explicitly keeps the input key after a successful HTTP result.
            user.access_key.zeroize();
            user.access_key = Some(old.to_owned());
        }
        validate_secret(&user.uid, 256).map_err(|_| protocol())?;
        validate_secret(user.access_key(), 16 * 1024).map_err(|_| protocol())?;
        if user.id_token.as_ref().is_some_and(|v| v.len() > 16 * 1024)
            || user
                .refresh_token
                .as_ref()
                .is_some_and(|v| v.len() > 16 * 1024)
        {
            return Err(protocol());
        }
        Ok(user)
    })
}
