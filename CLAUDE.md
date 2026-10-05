# CLAUDE.md

Guide for agents that work in this repository.

## What this is

`autumn-plugin-aws-lambda`: an Autumn (`autumn-web` 0.8) plugin that runs
an Autumn app on AWS Lambda. Design: loopback proxy
(`docs/adr/0001-loopback-proxy.md`). Planning: `docs/planning.md`.

## Layout

| Path | Content |
|---|---|
| `src/lib.rs` | Public API (4 items) and `__private` (items for tests only). |
| `src/plugin.rs` | `AwsLambdaPlugin`: options, startup decision (`plan`), start. |
| `src/runtime.rs` | Readiness wait, runtime thread, watchdog, `lambda_http` loop. |
| `src/proxy.rs` | `LambdaProxy`: Lambda request to upstream HTTP call. |
| `src/headers.rs` | Header rules, client IP, path encoding. |
| `src/timing.rs` | Invocation budget, backoff. |
| `src/upstream.rs` | Upstream address from `[server]` config. |
| `src/activation.rs` | When to start; Lambda env check; concurrency. |
| `src/error.rs` | `PluginError`. |
| `verification/core.rs` | Verus model and proofs of the pure core. |
| `verification/verify.sh` | Runs the proofs. |
| `tests/` | Unit, property, proxy, readiness, and end-to-end tests. |
| `examples/hello.rs` | Full app. The end-to-end tests run it. |

## Rules

- Process: SPEC, PROOF, RED, GREEN, REFACTOR. Write a test for each behavior.
- To change a pure function that has a Verus twin:
  1. Change the twin in `verification/core.rs`.
  2. Run `verification/verify.sh`.
  3. Update `tests/spec_conformance.rs`.
- Before a commit, run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`, and `cargo test`.
- Keep line coverage at 85% or more.
- Do not use `unwrap()` in `src/`. Do not use `unsafe`.
- Write comments and docs in ASD-STE100 style: simple words, active voice,
  one idea in each sentence. Keep them short.

## Important facts

- `AppBuilder::run()` binds the listener before startup hooks run.
- The `lambda_http` loop future is not `Send`. It runs on its own thread.
- The proxy does not return a Lambda invocation error. Failures before the
  response head are `502` or `504` responses.
- Autumn checks the `Host` header. End-to-end tests set
  `AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS`.
- Autumn `RequestIdLayer` ignores the incoming `x-request-id`. The plugin
  uses `lambda-runtime-aws-request-id`.
