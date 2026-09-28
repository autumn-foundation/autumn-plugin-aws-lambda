# CLAUDE.md

Guide for agents that work in this repository.

## What this is

`autumn-plugin-aws-lambda`: an Autumn (`autumn-web` 0.7) plugin that runs
an Autumn app on AWS Lambda. Design: loopback proxy
(`docs/adr/0001-loopback-proxy.md`). Planning: `docs/planning.md`.

## Layout

| Path | Content |
|---|---|
| `src/plugin.rs` | `AwsLambdaPlugin`: options, startup decision (`plan`), start. |
| `src/runtime.rs` | Readiness wait, runtime thread, `lambda_http` loop. |
| `src/proxy.rs` | `LambdaProxy`: Lambda request to upstream HTTP call. |
| `src/headers.rs` | Hop-by-hop removal, request ID. |
| `src/timing.rs` | Invocation budget, backoff. |
| `src/upstream.rs` | Upstream address from `[server]` config. |
| `src/activation.rs` | When to start; Lambda env check. |
| `verification/core.rs` | Verus model and proofs of the pure core. |
| `tests/` | Unit, property, proxy, readiness, end-to-end tests. |
| `examples/hello.rs` | Full app. The E2E tests run it. |

## Rules

- Process: SPEC, PROOF, RED, GREEN, REFACTOR. No behavior without a test.
- If you change a pure function with a Verus twin, change the twin in
  `verification/core.rs`, run the proofs, and keep
  `tests/spec_conformance.rs` in agreement.
- Before a commit: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`. Coverage must stay at 85% lines or more.
- No `unwrap()` in `src/`. No `unsafe`.
- Comments and docs: short, in ASD-STE100 style (simple words, active
  voice, one idea for each sentence).

## Important facts

- `AppBuilder::run()` binds the listener before startup hooks run.
- The `lambda_http` loop future is not `Send`. It runs on its own thread.
- The proxy never returns a Lambda invocation error. Failures are `502`
  or `504` responses.
- Autumn checks the `Host` header. E2E tests set
  `AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS`.
