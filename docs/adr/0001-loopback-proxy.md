# ADR 0001: Use a loopback proxy to connect Lambda to Autumn

- Status: Accepted
- Date: 2026-09-28

## Context

`autumn_web::AppBuilder::run()` builds the router, binds the listener,
and serves requests. It does not give the router to other code. A plugin
can only add hooks. `run()` binds the listener before it runs startup
hooks.

## Decision

The plugin adds one startup hook. When Lambda is detected, the hook
spawns a task. The task:

1. Waits until Autumn sets "startup complete".
2. Starts the `lambda_http` runtime loop.
3. Sends each event as an HTTP/1.1 request to the local Autumn listener.
4. Returns the response to Lambda.

```mermaid
sequenceDiagram
    participant AWS as Lambda Runtime API
    participant Loop as Plugin runtime loop
    participant App as Autumn server (127.0.0.1:port)
    AWS->>Loop: GET /invocation/next (event)
    Loop->>Loop: event to http::Request (lambda_http)
    Loop->>App: HTTP/1.1 request (loopback)
    App-->>Loop: HTTP response
    Loop->>Loop: remove hop-by-hop headers
    Loop-->>AWS: POST /invocation/{id}/response
```

## Consequences

Good:

- No change to `autumn-web`. All Autumn middleware and config stay active.
- The same binary runs outside Lambda. `Activation::Auto` does nothing
  there.

Bad:

- Each request uses a loopback TCP connection. The client keeps
  connections open, so the cost is small.
- Unix socket and TLS listeners are not supported. The plugin stops
  with a clear error.

## Alternatives

- In-process router: needs a new `autumn-web` API.
- `TestApp::into_router()`: skips production wiring.
- AWS Lambda Web Adapter: no access to the Lambda context.
