# Changelog

Version numbers follow MAJOR.MINOR.PATCH. Pre-release APIs are experimental.

## 0.1.0-alpha.3 - 2026-09-25

- Add an opt-in lazy `/accounts` email/password JSON source. Health/status and
  startup never load account passwords or trigger login. Missing files remain live;
  protected queries initialize one selected account/region with single-flight.
- Check regional role existence before login; automatically create only missing
  roles by default, with an option to disable creation. Require SDK-readiness
  confirmation, isolate persisted state, reuse saved sessions and never replay the
  triggering query or loop on unchanged failed inputs. Add SIGHUP rearming.
- Validate account-file schema, private permissions, selection and size bounds;
  keep passwords outside logs/snapshots and reject implicit account switching.

- Keep an unconfigured server/container alive in health-only mode. `/health` and
  `/healthz` return 200 without initializing or probing upstream clients. Log all
  missing required fields/files by name; business and readiness routes return 503.
- Preserve strict `check-config` validation and reject malformed configuration or
  unsafe API-key files. Add empty-container startup and graceful-shutdown checks.
- Reject dangling/nonregular session pointers before any login, and preserve the
  initialization failure category across repeated queries with unchanged inputs.
- Keep the existing HTTP `/v1` routes, query parameters and response contract.

## 0.1.0-alpha.2 - 2026-09-24

- Add operator `sdk-login`, `game-login`, `auth-status` commands, private login
  configuration, explicit SDK-readiness/account-creation flags and scoped snapshots.
- Add opt-in bounded game credential recovery from approved SDK authorization,
  single-flight/cooldown, no original-query replay, account identity checks and
  fail-closed persistence. SDK expiry requires explicit operator login.
- Add SIGHUP credential reload, authenticated readiness/status, correlation IDs
  and sanitized JSON diagnostics. Reject old authenticated cache after local blocks.
- Default HTTP responses to a recursive public-field whitelist; retain explicit raw
  mode and disable personalized recommendations in public mode.
- Establish the `/v1` compatibility baseline and pin its public OpenAPI contract.

- **Breaking HTTP change:** replace POST `/experimental/v1/...` queries with short
  GET `/v1/...` resource routes and URL parameters; remove old routes without redirects.
  Arrays use repeated keys and nested filters use dotted names. Preserve upstream
  gRPC, bearer authentication, protobuf JSON responses and no-store/cache behavior.
- Generate GET query parameters in OpenAPI and reject ambiguous parameters,
  invalid encoding, oversized URLs and GET request bodies before upstream work.

- Accept SDK RSA public keys with nonstandard Base64 line widths and no final
  newline, while preserving strict SPKI decoding and key-size limits.
- Decode integer SDK user IDs losslessly, including values above the JavaScript
  safe-integer range; reject fractional and overflowing numeric IDs.
- Add explicit private SDK authorization persistence and generation-checked game
  session export, compatible with static credential loading; no automatic saves.

## 0.1.0-alpha.1 - 2026-09-23

- Add descriptor-based protocol generation and protobuf JSON reflection.
- Add allowlisted read-query client with static credential injection and session isolation.
- Add opt-in raw HTTP queries, API key access, bounded cache and OpenAPI.
- Add offline protocol, loopback gRPC, session and HTTP/cache tests.
- Add redacted, origin/region-bound Android OneSDK callback import and explicit
  pre-login/game-login methods, outside the query allowlist and HTTP routes.
- Preserve native probe/login field differences and atomically rotate session
  generations only after validated login responses; never force device override.
- Add offline auth mapping, malformed-response, binding, cancellation and race tests.
- Add explicit SDK HTTP RSA, password and cached-key login primitives with
  recovered POST encoding/signing, instance-bound RSA challenges, bounded HTTPS
  transport and redacted pending results. No automatic game-session conversion.
- Add same-version OkHttp form vectors and loopback SDK HTTP regression tests.
- Fix Docker build inputs to include embedded SDK documentation; check the
  actual Docker context with an offline build before building the runtime image.
- Reject excessive scheduling intervals and cache lifetimes/capacities through
  direct Rust APIs, preventing duration-overflow panics and failed cache flights.
- Document security boundaries and the remaining RSA dependency advisory.
- Add native Linux amd64/arm64 container builds, offline container smoke tests,
  public GHCR publication and automated GitHub prereleases after CI passes.
- Cache Rust test artifacts and export cargo-chef dependency layers through
  architecture-scoped GitHub Actions BuildKit caches.
- Add matching manifest/lockfile/tag/changelog checks, OCI metadata, immutable
  digest release assets and a `moenotes-server --version` command.

No live API compatibility, complete SDK login flow, automatic renewal or stable public API is
claimed. No official `0.1.0` release has been made.
