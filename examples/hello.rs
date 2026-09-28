//! A small Autumn app for AWS Lambda.
//!
//! Local run: `cargo run --example hello`, then open <http://127.0.0.1:3000/hello>.
//! Set `LAMBDA_RESPONSE_MODE=streaming` to use streaming responses.

use autumn_plugin_aws_lambda::{AwsLambdaPlugin, ResponseMode};
use autumn_web::prelude::*;

#[get("/hello")]
async fn hello(headers: axum::http::HeaderMap) -> String {
    let rid = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    format!("hello from autumn (request {rid})")
}

#[autumn_web::main]
async fn main() {
    let mode = match std::env::var("LAMBDA_RESPONSE_MODE").as_deref() {
        Ok("streaming") => ResponseMode::Streaming,
        _ => ResponseMode::Buffered,
    };
    autumn_web::app()
        .plugin(AwsLambdaPlugin::new().response_mode(mode))
        .routes(routes![hello])
        .run()
        .await;
}
