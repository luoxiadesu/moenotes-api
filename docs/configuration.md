# Single-File Configuration

Since `0.1.0-alpha.4`, all manually entered server, region, device and SDK settings
can live in `config.toml`. Older images require the legacy file-based settings.
Account passwords remain in `/accounts/*.json`; session snapshots remain in a
persistent writable state directory. No passwords are accepted in the config.

Start from [config.example.toml](../config.example.toml). It is intentionally blank:
without required values, `serve` exposes `/health` and `/healthz` with HTTP 200
`{"status":"ok"}`, logs missing key names and keeps business routes at 503.
No SDK/game clients, login or upstream checks run in health-only mode. An absent
file or an empty file behaves the same way. Supply the config and restart to
activate queries; SIGHUP does not reread configuration.

## Settings

| Section/key | Purpose |
| --- | --- |
| `listen` | HTTP bind address; use `0.0.0.0:8080` inside a container. |
| `api_key` | Your HTTP bearer key, 32-4096 ASCII characters without whitespace. |
| `response_mode` | Keep `public` for shared callers; `raw` is trusted-operator-only. |
| `[session]` | Region, approved game HTTPS origin, platform and client/data versions. |
| `[accounts]` | Account directory, optional selected file, role creation and SDK readiness. |
| `[login]` | Persistent `state_dir`, writable by the service user. |
| `[login.context]` | Observed Android device model, OS, identifier and channel numbers. |
| `[login.sdk_http]` | SDK HTTPS base/allowlist, AppKey, country and optional SDK header. |
| `[login.sdk_http.common]` | SDK common request fields; values are strings. |
| `[recovery]` | Opt-in game-session recovery using saved SDK authorization. |

Use actual authorized device/SDK values, not arbitrary IDs. The repository and
image do not embed a service AppKey or operator device values. SDK common fields
are separate from game session metadata. Keep the existing request interval and
cache defaults unless there is a measured reason to change them.

## SDK Null Values

TOML has no `null`. Put every SDK common parameter either in the `common` table or
in `omit_common`, never both. The union must contain exactly the 20 known keys
listed in the template. Duplicates and unknown keys fail validation.

For example, `omit_common = ["adid"]` sends no `adid` field, matching a JSON
`"adid": null`. Conversely, `ad_ext = ""` is an explicit empty string and is sent
as an empty value. Omitting a key from both locations is incomplete configuration,
not an instruction to omit it upstream. This preserves the legacy SDK encoding.

## Files and Permissions

Only three mounts are needed in directory-login mode:

- `config.toml` to `/etc/moenotes/config.toml`, read-only.
- Account directory to `/accounts`, read-only, with `{"user":"EMAIL","password":"PASSWORD"}` files.
- State directory to `/var/lib/moenotes`, read-write and persistent.

The container runs as UID/GID 65532. Private files must be mode 0600 or stricter;
account/state directories must be mode 0700 or stricter and owned/accessed by that
user. A config containing inline secrets or device values is also a secret file:
no group/world access or symlink. Keep backups private and exclude it from Git and
images. Empty templates contain no secrets and can be readable while unconfigured.
Malformed TOML, conflicting sources, invalid nonempty keys and unsafe permissions
are errors, not health-only fallbacks. The config is limited to 64 KiB.

`check-config` validates the configuration offline. It does not validate passwords
or live sessions. Health/status never trigger login, and the first protected query
still starts lazy account loading and returns 503 until the caller retries after
completion. See [account lifecycle](accounts.md) for role creation and recovery.

## Compatibility

Existing deployments can keep their file-based settings unchanged, or migrate
each source independently:

| Inline setting | Legacy alternative |
| --- | --- |
| `api_key` | `api_key_file` |
| `[login.context]` | `login.context_file` JSON |
| `[login.sdk_http]` and `.common` | `login.sdk_http_file` JSON |

Providing both sources is rejected, even if the file path is empty. Relative
legacy paths still resolve against the config directory. Static `credentials_file`
and manual operator login remain available without `[accounts]`; do not combine
static credentials with managed login. Saved SDK/game snapshots are unchanged.
