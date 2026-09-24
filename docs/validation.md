# Validation Record

## 2026-09-24 Update

Current source includes GET `/v1` routing and SDK/session persistence. All 54
tests (36 client, 3 protocol, 14 HTTP/cache/query, 1 CLI) and two doctests pass,
along with formatting and all-target Clippy with warnings denied.
Query tests cover repeated array ordering/duplicates, exact 64-bit IDs, optional
presence, dotted filters, enums, invalid percent encoding/UTF-8, duplicate scalars,
unknown fields, size limits, GET bodies, removed routes, method rejection and
cache-key equivalence. OpenAPI declares all fifteen GET operations and parameters.

An authorized smoke test with a saved game session returned HTTP 200 for profile
and announcement GETs, MISS then HIT for the profile, 401 without a key, 400 for
invalid query/body, 404 for the old route and 405 for POST/HEAD. Only two upstream
query calls were needed; no login or game-state mutation was performed. The test
server was stopped afterward. See [live scope](live-validation.md) for prior
authentication and query coverage. No new image or release has been published.

## Initial Validation

Local validation on 2026-09-23, Linux x86_64 (WSL2), pinned Rust 1.98.1.
This record covers the initial framework, SDK exchange, SDK HTTP increments and
local preparation of `0.1.0-alpha.1`, not a stable release.

## Passed

- `cargo fmt --all --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace --all-targets`: 42 tests passed
  (30 client, 3 protocol, 8 HTTP/cache, 1 CLI process integration).
- `cargo test --locked --workspace --doc`: two login integration examples compiled.
- `cargo build --locked --release -p moenotes-server`
- `cargo doc --locked --workspace --no-deps`
- Seven Python release-script tests, including version/tag/lockfile consistency,
  stable/prerelease handling, public-package enforcement and manifest validation.
- GitHub Actions workflow syntax checked with actionlint 1.7.12.
- Loopback offline example: health response, authenticated query, MISS followed
  by HIT with the same original fetch timestamp. The example was then stopped.
- Protocol snapshot SHA-256 matches the provenance notice.
- `sh scripts/check-build-inputs.sh`: Docker's actual `inputs` stage exported
  under `.dockerignore`, then the complete exported workspace checked offline.
  Embedded SDK docs and the descriptor/lockfile are present; target/secrets are absent.

Pre-commit review fixed omitted Docker documentation inputs and direct-library
configuration overflow in scheduling/cache expiry. New regression tests failed
before the fixes and pass afterward. CI now covers all targets, doctests,
documentation and the build-input check with read-only repository permissions in
the test job. Tag-only image/release publication has narrowly scoped write access.

The client tests exercise all 18 query methods plus two explicit login methods through a local HTTP/2 gRPC
mock, including 15 business-query request encodings and conditional headers. Other
tests cover protobuf JSON integers/presence/enums/maps/unknown wire fields,
initial and trailing errors, secret redaction, origin binding, Unix secret-file
permissions, cancellation, deadlines, queue limits, session replacement and
blocked sessions. Server tests cover all 15 routes, default-disabled behavior,
authentication, cache TTL/capacity/byte budget/coalescing, full request keys,
stale-generation rejection, errors and CLI startup/shutdown.

Auth tests additionally cover the callback field mapping, pre-login omission of
idToken/client-version metadata, absence of inherited credentials/forced-device
headers, ignoring response device_id, malformed responses, cross-origin/region
rejection, no retries, cancellation/timeouts and competing login generations.

SDK HTTP adds 11 tests for the three POST methods, account-type fields,
root/prefixed paths, headers, signing, RSA encryption, old cached-key preservation,
strict/redacted responses, limits, no redirects/retries, instance binding,
cancellation, timeout, busy admission and rate waiting. Sixteen synthetic form
vectors were generated with the published OkHttp 3.12.13 / Okio 1.15.0 artifacts,
matching the sample's version. The small oracle's Uri.encode step models its
allowlist; it does not execute Android framework code. The RSA roundtrip test
uses one Rust library, not an independent Android implementation.

No game/SDK-server connections or real credentials were used. No
TLS connection to the game, live SDK/login flow or real account response was
validated. gRPC/SDK HTTP mocks use loopback plaintext; production transports
require HTTPS. Tests do not prove live TLS interoperability or account acceptance.

## Container Validation

Earlier local attempts timed out while downloading the Rust builder image.
This gap was closed on 2026-09-23 by GitHub Actions run
[`35872332332`](https://github.com/luoxiadesu/moenotes-api/actions/runs/35872332332)
at commit `645662d`: the full test gate and native Linux amd64/arm64 builds passed.
Each runtime image passed `--version`, UID/GID 65532, private synthetic config,
health, unauthenticated OpenAPI rejection, default-disabled query routes,
`check-config` and graceful shutdown checks. This was a main-branch validation run,
not a registry publication. Subsequent release runs are linked from GitHub Releases.

The cold run compiled cargo-chef, cooked locked dependencies, built the real
application and exported architecture-specific GHA caches. No Docker daemon
settings or unrelated local containers were changed. Container tests still do not
validate game connectivity or live authentication.

## Pending

Locked dependency advisories were inventoried from RustSec and reviewed manually;
see [security boundaries](../SECURITY.md). The RSA private-key timing advisory is
still present upstream, although production uses public-key encryption only.
No successful cargo-audit run is claimed. A staged-file scan found no recognized
secret patterns, private paths or executable analysis artifacts; this is not a
guarantee of legal redistribution rights or exhaustive secret detection.

Complete SDK authorization/renewal, successful authorized game requests, current versions/master,
real server limits, field visibility and the public-response projection policy
remain separate future work. The public Rust/HTTP APIs are not frozen.
