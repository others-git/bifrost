# Bifrost — project directives for Claude

## Test coverage is mandatory

Every public function, method, and non-trivial private helper must have test coverage.
There are no exceptions. This is not negotiable.

### Where tests live

| What | Where |
|---|---|
| Pure logic (crypto, models, color math, backoff) | `#[cfg(test)]` module at the bottom of the same file |
| Provider HTTP behaviour (Hue, Govee, future integrations) | `#[cfg(test)]` module in the provider file, using `wiremock` |
| API layer (Axum routes, auth, DB) | `tests/api.rs` using an in-memory SQLite app fixture |
| Cross-cutting integration | `tests/` as named files |

### Rules

- **New provider?** The `LightProvider` impl AND the `ProviderFactory::build` path must both be covered by wiremock tests before the code is considered done.
- **New API route?** At minimum: happy path + unauthenticated request returns 401.
- **New crypto helper?** Roundtrip test + at least one failure-mode test (wrong key, tampered data).
- **The full CI gate must pass locally** before any change is considered complete — CI runs all three and fails on any:
  ```
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```
  Running only `cargo test` is not enough; fmt and clippy (with `-D warnings`) are equally blocking.
- Do not silence warnings with `#[allow(dead_code)]` to make tests pass. Fix the code.

### Test style

- Prefer real behaviour over mocks. Use `wiremock` for external HTTP; use real in-memory SQLite for DB tests.
- Test public contracts, not implementation details. Avoid testing private internals directly.
- Each test has one clear assertion purpose. Name it after the behaviour: `discover_returns_empty_list_when_bridge_has_no_lights`, not `test1`.
- Inline test helpers are fine; large shared fixtures go in `tests/helpers.rs`.

## Other directives

- The `ProviderRegistry` is the single place where provider types are registered. Do not add provider-type match arms anywhere else.
- Credentials are encrypted with AES-256-GCM before persisting. Never store plaintext credentials in the DB.
- The Hue connection manager must be the only code that reconnects to the bridge SSE stream. Do not open a second stream anywhere.
