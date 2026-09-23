# Experimental HTTP API

Rust cache options are limited to a one-hour TTL and 16,384 entries, matching CLI
configuration limits. Invalid options fail before upstream work is scheduled.

No response or route stability is promised before the first release. The service
is designed for one operator-configured game session per process. Use separate
processes and separate configuration for separate regions. All authorized HTTP
callers use that same upstream identity; API keys are not game credentials.

## Access and Data Boundary

`GET /healthz` is unauthenticated and reports process liveness, not upstream health.
`GET /openapi.json` requires a bearer API key and describes the experimental routes
even when disabled. All query routes are disabled unless
`enable_experimental_raw = true` and always require `Authorization: Bearer ...`.
Use the configuration/secret files locally; there is no remote management endpoint.

HTTP keys must be 32-4096 ASCII non-whitespace characters, stored separately from
game credentials. Secret files may have a trailing newline. The process retains a
SHA-256 hash of the HTTP key and compares hashes in constant time. Restart the
process to rotate a key or reload configuration. Never pass keys as URL parameters.

Raw response mode is deliberately not a shared public projection. Account-specific
fields are retained, including recommendation results and favorite status. Only
enable it for trusted callers until real response comparisons establish a suitable
public-field policy. Announcement HTML is returned as a JSON string, never executed;
consumers must not treat it as sanitized HTML. No CDN authorization values are added.

## Queries

All routes use POST with an `application/json` protobuf request body. This avoids
ambiguous array query parameters; these are read queries, not gameplay writes.
64-bit values should be decimal strings. Unknown fields are rejected. Arrays retain
their order and duplicates; no sorting/deduplication or UI-specific mutation occurs.

| Route under `/experimental/v1/` | Body fields |
|---|---|
| `announcements/detail` | `id` |
| `announcements/list` | `selectedTab` (ALL, OPENING_EVENTS, BUG; default ALL) |
| `arena/ranking` | `arenaSeasonId`, optional `bandId`, `rankingStart`, `rankingEnd` |
| `arena/deck-trend` | `musicId`, `arenaSeasonId` |
| `circles/detail` | `circleId` (uint64) |
| `circles/recommendations` | `{}` |
| `circles/search` | `options` containing `name`, `memberRange`, `joinRule`, `playStyle` |
| `events/challenge-ranking` | `challengeMusicId` |
| `events/deck` | `playerId` (string), `eventId` |
| `events/ranking` | `eventId`, `ranks` (int32 array) |
| `profiles/find` | `playerProfileId` |
| `gacha/probability` | `gachaId`, optional `selectedPickUp` (int64 array) |
| `music/ranking` | `musicId` |
| `profiles/favorite-status` | `playerId` (string) |
| `profiles/batch` | `accountIds` (int64 array) |

There is no HTTP route for arbitrary RPCs, session credentials, Whoami, raw server
discovery or version management. Client-library support methods are independent.
The OpenAPI schema is generated from the bundled descriptor; it describes protobuf
fields, while the following local admission limits are additional policies.

Request bodies are limited to 64 KiB; at most 64 authenticated HTTP handlers can
run concurrently. The cache admits at most 32 different inflight keys; duplicate
requests share the same operation. Additional work receives HTTP 429. Query
validation uses the [local client limits](client.md); none are asserted server limits.

## Responses, Cache and Errors

Successful response bodies contain the upstream protobuf JSON object directly,
without a `data` envelope. Defaults are omitted per protobuf JSON; int64/uint64
are strings, maps are JSON objects, known enums are names and unknown enums are
numbers. Protobuf unknown fields remain available in the Rust response but are not
invented as JSON keys. No account-field filtering, enrichment or UI formatting.

Headers:

- `X-Moenotes-Fetched-At`: original successful fetch time, Unix milliseconds.
- `X-Moenotes-Cache`: `MISS`, `HIT` or `COALESCED`.
- `Cache-Control: no-store`: callers/proxies must not turn raw data into a shared cache.

Internal caching defaults to 15 seconds, 1024 entries and a 64 MiB serialized-JSON
budget (an accounting bound, not an exact process RSS limit). Eviction is least
recently touched. Keys include the random session generation, method and complete
encoded request. Region/origin/identity are immutable within a generation.
Errors are never cached, expired successes are never served on failure, and replacing
the session invalidates old entries. A caller disconnect does not abort a shared
inflight query; it continues within the client deadline, which can still fill cache.
Graceful process shutdown cancels pending work. Process restart clears all cache.

Existing unexpired success entries may remain available after a later upstream
error on another query in the same session; their original fetch time is preserved.
They are not evidence that the blocked upstream session has recovered.

Error body: `{"error":{"kind":"maintenance"}}`. No raw upstream body or status
message is exposed. HTTP 401 means the HTTP key failed, **not** game authentication.

| HTTP status | Meaning |
|---|---|
| 400 | Invalid JSON/query, unsupported parameter or local validation failure |
| 401 | Missing/invalid/duplicate bearer authorization |
| 404 | Unknown or disabled route |
| 429 | Local HTTP/inflight/upstream admission limit reached |
| 502 | Upstream transport, protocol or other business failure |
| 503 | Missing/rejected game session, maintenance, version/device conflict, cancellation or session change |
| 504 | Operation deadline exceeded |

The gateway does not translate unknown business errors into not-found or empty
success and does not initiate authentication, retries or background polling.

## Deployment

Loopback is the default. For remote access, terminate TLS at a trusted reverse proxy,
limit request rate and connection/body-read timeouts there, and restrict network
access. No browser CORS permission is granted. Never enable request-body/header
logging at a proxy without redaction. Live visibility, account isolation semantics,
query depth, refresh rate and server error behavior still need authorized validation.
