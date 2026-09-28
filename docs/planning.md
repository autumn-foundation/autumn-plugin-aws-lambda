# Planning: Autumn plugin for AWS Lambda

This document records the planning phase. It uses three methods:
brainstorming, reverse brainstorming, and six thinking hats.

## Goal

Run an Autumn (`autumn-web` 0.7) application on AWS Lambda.
Add one line to the app: `.plugin(AwsLambdaPlugin::new())`.
The same binary must also run as a normal server outside Lambda.

## Facts from research

- `AppBuilder::run()` builds the router, binds the listener, and serves.
  It does not give the router to other code.
- `AppBuilder::run()` binds the listener **before** it runs startup hooks.
- `AppState::probes().is_startup_complete()` becomes `true` after all
  startup hooks finish.
- `lambda_http` changes Lambda events (API Gateway REST and HTTP, ALB,
  Function URL, VPC Lattice) into `http::Request` values.
  It changes `http::Response` values back into Lambda responses.
- `lambda_runtime` needs `AWS_LAMBDA_RUNTIME_API` and other Lambda
  variables. It panics if they are missing.

## 1. Brainstorming

Possible designs:

1. **Loopback proxy.** Autumn serves on `127.0.0.1:<port>`. A startup
   hook starts the Lambda runtime loop. The loop sends each event to the
   local server over HTTP and returns the response.
2. **In-process router.** Get the `axum::Router` and call it directly.
   `run()` does not expose the router, so this needs a change in
   `autumn-web`.
3. **Test router.** Use `autumn_web::test::TestApp::into_router()`.
   This does not use the production setup (config, database, middleware).
4. **Unix socket proxy.** Same as (1), but on a Unix socket.
5. **Lambda Web Adapter.** Use the AWS layer. This needs no code. But it
   is a separate process, and it does not know the Autumn startup state.

Feature ideas:

- Detect Lambda from `AWS_LAMBDA_RUNTIME_API` (mode `Auto`).
- Support buffered and streaming responses.
- Use the invocation deadline as the upstream timeout.
- Send the Lambda request ID to the app.
- Remove hop-by-hop headers in both directions.
- Wait until the app is ready before the first event.
- Support Lambda Managed Instances (concurrent invocations).

## 2. Reverse brainstorming

Question: "How can we make this plugin fail?" Each answer gives a guard.

| How to cause failure | Guard |
|---|---|
| Start the runtime loop on a laptop. `lambda_runtime` panics. | `Activation::Auto` starts the loop only when `AWS_LAMBDA_RUNTIME_API` is set. |
| Connect to `0.0.0.0`. | Map unspecified addresses to loopback. |
| Configure a Unix socket or TLS. The proxy cannot connect. | Stop at startup with a clear error. |
| Send `transfer-encoding: chunked` to API Gateway. | Remove hop-by-hop headers from the response. |
| Send a wrong `content-length` after base64 decode. | Remove `content-length`. The client sets it again from the body. |
| Block the startup hook. Other hooks never run. | Spawn the loop. Return from the hook immediately. |
| Let the upstream call run past the Lambda deadline. | Use `deadline - now - margin` as the timeout. Return `504`. With no time left, do not call. |
| Return a Lambda error when the app is down. Async callers retry. | Return a `502` HTTP response. |
| Lose the `Host` header. `trusted_hosts` rejects the request. | Keep the original `Host` header. |
| Change the client IP. | Set `x-forwarded-for` to the IP that AWS saw. Remove `x-forwarded-host`, `x-real-ip`, `forwarded`. |
| Delete `Host` with a `Connection` header. | On requests, remove only the fixed hop-by-hop list. |
| Send `/a/%2e%2e/admin` to reach `/admin`. | Send the raw event path. Do not remove dot segments. |
| Lose repeated response headers. | Join repeated headers, except `Set-Cookie`. |
| The runtime loop stops. The process stays alive and gets no events. | Log the error. Exit with code 1. Lambda starts a new environment. In concurrent mode, check the Runtime API every second. |
| Register the plugin twice. Two loops poll for events. | The default `Plugin::name` makes the second call a no-op. |

## 3. Six thinking hats

**White hat (facts).** `run()` owns the router. Startup hooks run after
bind. `lambda_http` does the event conversion. Proxy overhead on loopback
is small compared to Lambda latency.

**Red hat (feelings).** Users want "one line and it works". A proxy
inside the process feels strange, but AWS Lambda Web Adapter uses the
same pattern, so it is familiar.

**Black hat (risks).** Two copies of each body in memory. Extra latency
on loopback. Config mistakes (Unix socket, TLS, hostname). The `prod`
profile of Autumn has strict checks (signing secret, trusted hosts) that
can fail on cold start. Graceful shutdown on Lambda has little time.

**Yellow hat (benefits).** No fork of `autumn-web`. All Autumn middleware
and config stay active. The same binary runs on a local computer and on
Lambda. The
pure parts (activation, address, headers, path, budget) are easy to test
and to prove.

**Green hat (ideas).** Streaming mode for Function URLs. Concurrent mode
for Managed Instances. Later: Unix socket upstream when `autumn-web`
gives a router hook.

**Blue hat (process).** Decision: design (1), loopback proxy.
Order of work: Verus spec and proof for the pure core, then failing
tests (RED), then code (GREEN), then refactor. Then agents review the
code for different concerns, and we fix the findings.

## Decision

Use the loopback proxy. See
[ADR 0001](adr/0001-loopback-proxy.md).

## Scope

In scope:

- `AwsLambdaPlugin` with `new()` and fluent options.
- `Activation`: `Auto`, `Always`, `Never`.
- `ResponseMode`: `Buffered`, `Streaming`.
- Upstream from Autumn `server.host` and `server.port`, or an override.
- Readiness wait, deadline timeout, header and path rules, request ID.
- Verus proofs for the pure core.
- Unit, property, integration, and end-to-end tests.

Out of scope:

- Unix socket and TLS upstreams (clear error at startup).
- Non-HTTP Lambda events (SQS, S3, and so on).

## Review

After REFACTOR, five agents reviewed the code: correctness, security,
lifecycle, API and docs, and tests and proofs. The fixes are in the
"Reverse brainstorming" table above. The review also added Verus proofs
for path encoding and the zero-budget rule.
