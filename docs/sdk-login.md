# SDK-to-Game Login

The Rust library supports an **explicit, offline-tested exchange** from an already
authorized Android OneSDK login callback to game credentials. It does not perform
the complete SDK login flow or automatic renewal. Separate [SDK HTTP primitives](sdk-http.md)
cover RSA, password and cached-key requests, but return pending results without
completing SDK initialization, challenge UI or post-login agreement checks.
There is no login route in the HTTP gateway and no login CLI command.

## Minimal Integration

```rust,no_run
use moenotes_client::{CancellationToken, Client, ClientOptions, QueryClient, SessionConfig};
use moenotes_client::auth::{AndroidLoginContext, LoginReceipt, SdkAuthorization};

async fn authenticate(
    config: SessionConfig,
    authorized_callback_json: &[u8],
    context: AndroidLoginContext,
) -> Result<(Client, LoginReceipt), moenotes_client::ClientError> {
    let sdk = SdkAuthorization::from_callback_json(&config, authorized_callback_json)?;
    let client = Client::new(config, None, ClientOptions::default())?;
    let receipt = client.login_with_sdk(
        client.generation(), &sdk, &context, CancellationToken::new(),
    ).await?;
    Ok((client, receipt))
}
```

The integration supplies an approved HTTPS origin/region, `platform = "android"`,
the actual client version, device model, operating-system string, Unity device
identifier and numeric SDK channel/brand/area IDs. Values are not read from a device
or inferred from region names. Do not substitute `User.channelId` for the global
channel ID. Do not print callbacks, generated login protobufs or device identifiers.

The callback format is OneSDK's camelCase JSON: required nonempty string `uid` and
`accessToken`, with optional string `idToken`. Unknown fields are ignored, not
retained. Missing/null `idToken` becomes empty by local compatibility policy; that
is not a claim about all native JSON deserializers or server acceptance. SDK
authorization is bound to the importing config's normalized origin and region.
Binding is a local disclosure safeguard, not cryptographic token validation.

Local defensive limits: 64 KiB callback, 256-byte uid, 16 KiB per token,
1,024 bytes per nonempty device-context string. UID must be valid ASCII metadata.
These are library limits, not recovered official server constraints. Importing is
memory-only; the integration remains responsible for its input buffer and storage.

## Recovered Mapping

Evidence is the Android 1.0.1 native login builder at `0x59e448c`, the server probe
at `0x5e7a65c`, and the APK's OneSDK/GS Java bridge. No live acceptance is claimed.

| Request field | Source |
| --- | --- |
| `sdkUid`, `sdkAccessToken` | Callback `uid`, `accessToken` |
| `idToken` | Callback `idToken` for login; omitted by pre-login probe |
| `platform` | Native Android uint32 value `0`, not the metadata string |
| `deviceModel`, `operatingSystem` | Explicit context |
| `clientVersion` | Session config |
| `uuid.identifier` | Explicit Unity device identifier |
| `uuid.adId` | Empty in this native default implementation |
| `clientPackage` | `com.bilibili.sirius` |
| `globalChannelId`, `brandId`, `areaId` | Explicit numeric SDKChannelInfo values |
| `initial_data_group` | Not assigned by either recovered builder |

Both methods send `x-platform`, a fresh `x-request-id`, and SDK uid as
`x-player-bid`. Only final login sends `x-client-version` metadata; both include
the version in the request body. Existing game credential, device ID and version
pair headers are deliberately not inherited. No device-override header is sent.

`pre_login_with_sdk` returns only `isAccountCreated` and does not install a session
or automatically call login. Native probe uses a five-second deadline and parses
channel strings with TryParse; this library instead uses the configured operation
deadline and typed u32 inputs. It does not reproduce native UI/server selection.

## Session Safety

`login_with_sdk` sends exactly one RPC. **The official RPC can create a game
account or affect another device's session.** Obtain the account owner's approval
before calling it. Pre-login is not a transaction guaranteeing no account creation.
Cancelling, timing out or losing a response cannot roll back upstream effects.

A successful, well-formed response must contain a nonempty metadata-compatible
player ID and credential. They are installed atomically into a new generation;
the old generation is cancelled. A stale result cannot overwrite a replacement.
Failed/malformed/cancelled results do not replace credentials. Authentication,
version and device errors can mark the old generation blocked.

Installed BID is the SDK uid. Response `device_id` is deliberately not installed:
the native success path calls `SetupCertification(id, credential, null)` and then
`SetPlayer`. `profile_id` is not used as a player ID. The receipt returns only the
new generation and raw uint32 `isNewUser`, never a credential-bearing protobuf.
`session_status` remains a conservative local observation, not a perpetual
validity guarantee. No Whoami, player-data load or disk save is implicit.

Login shares the client's serial queue, admission limit, start interval, timeout
and cancellation. Version/device blocks require explicit session replacement;
explicit login may recover an authentication rejection. There is no automatic
retry, Aegis queue polling, CAPTCHA solver, SDK refresh or forced device takeover.
Queue/full business errors remain `Business`, with codes available only through
the existing explicit diagnostic accessor; queue-position headers are not exposed.

SDK authorization, device context and installed credentials have redacted Debug;
owned secret fields and temporary encoded buffers are zeroized where practical.
Protobuf reflection, HTTP/2 and caller-owned allocations are not guaranteed wiped.
Custom transports receive sensitive login bodies and must not log them. Generated
protobuf types still have ordinary Debug; they are not secret-safe containers.

## SDK Findings and Remaining Work

Java calls `OneSDK.login`, dispatches through `RealGameSdkProxy` to the bundled
platform9626 GS adapter, and returns `Gson.toJson(User)` to Unity. GS `access_key`
becomes Bundle `access_token`, then OneSDK `accessToken`; `id_token` becomes
`idToken`. The adapter drops GS expiration and refresh-token fields. OneSDK
`userState` does not directly match the C# `isNewUser` member.

The cached-login branch is gated by SDK config and a stored user. It posts the
existing `access_key` to `cache.login`, may add a CAPTCHA ticket or payment-license
voucher, then explicitly preserves that access key on success before subsequent
agreement checks. This is not evidence of refresh-token rotation. SDK HTTP calls
also have common parameters/signing, separate from the game's gRPC protocol.

Complete password/social/guest flows, SDK initialization/configuration, actual signing
configuration, challenge/consent handling, token lifetime/revocation and live
account/session behavior remain unimplemented or unverified. No app signing key,
real callback, credential, decompiled SDK source or account fixture is included
in this project. Do not infer production readiness from offline mock success.
