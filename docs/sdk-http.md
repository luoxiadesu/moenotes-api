# SDK HTTP Login Primitives

`moenotes_client::sdk_http` implements three explicit GS SDK POST operations from
the Android 1.0.1 sample. They are offline-tested library primitives, **not a
complete or live-verified SDK login workflow**. No HTTP gateway routes or CLI
login commands are added. Account owners must authorize their use.

| Method | Path | Business fields |
| --- | --- | --- |
| `fetch_rsa` | `/gapi/client/rsa` | None |
| `password_login` | `/gapi/client/login` | `user_id`, encrypted `pwd`, account type/country when applicable |
| `cache_login` | `/gapi/client/cache.login` | Existing `access_key` |

Password and cache calls accept optional nonempty `ticket` and
`third_payment_voucher`. `PasswordAccountType::Legacy` omits `country_id` and
`user_id_type`; `Email` sends type `0`, `Phone` sends `1`. Country IDs are explicit
u32 inputs, not inferred dialing prefixes. An optional `CurrentSdkUser` adds
`uid` and `mid`; these are SDK identifiers, not game player/device identifiers.

## Configuration

Supply `SdkHttpConfig`, `CommonParameters`, `AppKey`, `SdkHeaders` and
`SdkHttpOptions` to `SdkHttpClient::new`. Nothing is extracted from an APK, device,
account file or the game client's credential provider. No real AppKey is included.
The operator must supply authorized runtime configuration.

The exact normalized HTTPS base must appear in `allowed_base_urls`. Only a root
base or `/passport` base is accepted, without userinfo, query or fragment. The
sample's default channel uses root bases: **do not automatically add `/passport`**.
Some other channel/configuration branches use a prefix. No SDK host is inferred
from a game gRPC origin, and discovery never forwards credentials automatically.

`CommonParameters::new` requires all 20 keys in `COMMON_FIELDS`, each explicitly
`Some(value)` or `None` (native null omission):

```text
game_id server_id merchant_id app_ver sdk_ver channel_id platform platform_type
net operators model pf_ver udid dp adid lang sdk_log_type ad_ext time_zone isRoot
```

`timestamp` is generated as Unix milliseconds at request construction;
`web_code` is `6`. In this sample, GS SDK version is `4.2.12` and its platform
name is `google`, distinct from game gRPC metadata `android` and the native login
request's numeric platform `0`. The library does not manufacture device IDs or
guess runtime values from these constants.

## Minimal Library Use

This example uses caller-supplied configuration and a password object. It compiles
without making network requests during documentation tests.

```rust,no_run
use moenotes_client::CancellationToken;
use moenotes_client::sdk_http::{
    AppKey, CommonParameters, PasswordLogin, PendingSdkLogin, SdkError,
    SdkHeaders, SdkHttpClient, SdkHttpConfig, SdkHttpOptions, SdkReply,
};

async fn password_attempt(
    config: SdkHttpConfig,
    parameters: CommonParameters,
    app_key: AppKey,
    password: &PasswordLogin,
    cancel: CancellationToken,
) -> Result<SdkReply<PendingSdkLogin>, SdkError> {
    let sdk = SdkHttpClient::new(
        config, parameters, app_key, SdkHeaders::default(), SdkHttpOptions::default(),
    )?;
    let rsa = match sdk.fetch_rsa(None, cancel.clone()).await? {
        SdkReply::Success(rsa) => rsa,
        SdkReply::Rejected(reason) => return Ok(SdkReply::Rejected(reason)),
    };
    sdk.password_login(&rsa, password, None, cancel).await
}
```

Each method sends at most one request. `password_login` does not fetch RSA
automatically. Challenges are bound to the immutable client instance that fetched
them; another client cannot reuse one. No challenge expiration is inferred.
An explicit `CacheLogin` can instead be passed to `cache_login`.

## Wire Contract

Requests use `application/x-www-form-urlencoded`, User-Agent
`Mozilla/5.0 BSGameSDK`, `Api-Version: 1` and a fresh UUID `X-Game-Request-Id`.
Nonempty `trace_id` and `one_sdk_version` add `X-Game-Trace-Id` and `one-sdk-ver`
only when configured. No game credential or device-override metadata is inherited.

Password business fields pass through Android `Uri.encode`, then OkHttp
`addEncoded`. RSA/cache business fields and all common parameters go directly
through `addEncoded`. Consequently, literal `+` in a cache/common value means a
space after decoding, and `%2B` means a plus; password business fields preserve
both literally. Literal tab/newline/form-feed/carriage-return are stripped only
in the already-encoded mode. Do not apply a generic form encoder to these values.

Signing reads decoded form values, resolves duplicate names using the last value,
sorts names, excludes `item_name` and `item_desc` case-insensitively, concatenates
values without separators, appends AppKey and computes lowercase MD5. This is
the recovered **POST** contract, not a GET/multipart signer. Wire field order
does not reproduce Java HashMap iteration order.

Password encryption is UTF-8 `hash + password`, RSA/ECB/PKCS1Padding (PKCS#1 v1.5),
then standard padded Base64. The public key is SPKI PEM. These legacy primitives
are reproduced for compatibility, not recommended for new protocols. Production
code performs public-key encryption only, never private-key decryption.

## Pending Results

HTTP status must be 2xx excluding 204/205. JSON must have an integer `code`;
`0` requires valid nonnull `data`. Nonzero codes return `SdkReply::Rejected`.
Upstream `message` is discarded. Error Debug/display never includes response
bodies, URLs, passwords or tokens.

`PendingSdkLogin` retains uid, access key, optional id token, raw `expires` and
refresh-token presence. Cache success preserves the submitted access key even
when the response contains a different key; it is **not token rotation**.
Expiry units, epoch, validity and refresh behavior are not inferred.

HTTP success is followed by native agreement/configuration and other UI checks.
There is deliberately no conversion from `PendingSdkLogin` to `SdkAuthorization`
or a game session. Use [SDK-to-game login](sdk-login.md) only after independently
completing required SDK authorization. The pending result is a limited projection,
not sufficient for implementing every native post-login check; other user fields
are discarded.

`200007` identifies a CAPTCHA branch, `800011` an account-restore confirmation,
and `-101` a cached-authentication rejection branch. The library never solves a
challenge, fetches/opens an upstream URL, confirms restoration or retries. Failure
data is available through `sensitive_data_json()` for explicit integrations; it
is untrusted, may contain credentials, and must not be logged or forwarded to the
public gateway. It remains raw JSON, not Gson's string-map coercion.

## Safety and Differences

- Required TLS validation; no plaintext production mode, redirects, proxy
  inheritance, retries, cookies, automatic host failover or credential persistence.
- One network operation per instance; competing calls return `Busy`. Default
  minimum start interval is one second. Multiple instances have no global limiter.
- Default ten-second timeout covers rate waiting and HTTP I/O, not bounded local
  RSA encryption or JSON parsing. Cancellation cannot undo upstream effects.
- Local caps: 64 KiB encoded request, 256 KiB response including chunked bodies,
  4,096 bytes per common value/AppKey, 1,024 per user ID, 4,096 per password,
  16 KiB per token/ticket/voucher, 256 per uid/mid, 1,024 per optional header,
  8,192 per RSA PEM, 1,024 per hash. RSA keys must be 1,024-4,096 bits and
  `hash + password` must fit modulus bytes minus 11. These are defensive limits,
  not recovered official constraints.
- Required uid/access key must be nonempty strings. Missing/duplicate/mistyped
  required envelope fields fail instead of emulating all Gson defaults/coercions.
  Invalid UTF-8 percent escapes are rejected instead of Java replacement characters.
  Optional current-user null strings are not emulated. Unknown JSON fields are
  ignored. Configuration is immutable; recreate the client to update it.
- Secret-bearing types have redacted Debug and best-effort zeroization. Caller
  strings, deserialization scratch space, digest internals and HTTP buffers are
  not guaranteed wiped. Sensitive accessors expose values only for authorized
  integration use; do not serialize or log them.

SDK initialization/remote configuration, OTP/social/guest login, complete consent
and challenge UI, token refresh/revocation and live SDK/game acceptance remain
unimplemented or unverified. No official-service request was made to validate
this module. See [validation](validation.md) for offline test scope.
